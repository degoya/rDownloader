//! Searching the defined indexers and taking hits into the LinkGrabber (RD-180-19).
//!
//! **One request per search and per page.** Some indexers cache an answer for ten minutes on
//! their side (omgwtfnzbs says so), and every request counts against a daily limit, so nothing
//! here polls, retries or pages on its own: a search asks each chosen indexer once, and the next
//! page is the person's next request.
//!
//! **The key never leaves.** A hit's download address usually carries the API key, so the
//! address a client gets has the key replaced by [`KEY_PLACEHOLDER`]; the grab puts it back --
//! only for an address on the indexer's own server, or it would be a way to send the key
//! anywhere. An address elsewhere is fetched without the key, under the address guard the
//! LinkGrabber's proposed links keep to.
//!
//! **A hit becomes an NZB import through the upload's own path**, `store_nzb_import`: the same
//! parser, password convention and review list a dropped `.nzb` takes. A torrent hit from a
//! Torznab indexer (Jackett, Prowlarr; RD-1100-03) becomes a LinkGrabber package the way an
//! uploaded `.torrent` or a pasted magnet does.
//!
//! **A typed search** (`t=tvsearch`, `t=movie`; RD-1100-03) carries the ids the person typed;
//! nothing here looks one up, and nothing asks `t=caps` first -- the interface does that once
//! and offers only the types an indexer answers, and an indexer asked anyway refuses in its own
//! outcome.

use std::collections::BTreeMap;

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use rd_core::{CategoryId, Indexer, IndexerId, IndexerSearch};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, dto::MessageResponse, error::ApiError};

mod bodies;
mod grab;
mod typed;

pub use bodies::*;
pub use grab::*;
/// The search function a request names (RD-1100-03), for the MCP tool that builds one.
pub use rd_subscription::IndexerSearchType;

/// What stands in for the API key in an address handed to a client.
///
/// Unreserved characters only, so it survives in a query and in a path segment unencoded.
pub const KEY_PLACEHOLDER: &str = "rdownloader-indexer-key";
/// Results a search page asks for when the request names no limit.
const DEFAULT_SEARCH_LIMIT: u32 = 100;
/// Deepest offset a search may ask for; far beyond what any indexer pages through.
const MAX_SEARCH_OFFSET: u32 = 100_000;
/// Most hits one grab takes.
pub const MAX_GRAB_ITEMS: usize = 50;
/// How long one indexer may take to answer a search.
const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Searches one indexer or every enabled one (RD-180-19).
///
/// Answers `200` whenever the request itself was valid: an indexer that refused or could not be
/// reached is reported in its own outcome with a stable code, and the others' hits still come.
#[utoipa::path(
    post,
    path = "/api/v1/indexers/search",
    tag = "indexers",
    request_body = IndexerSearchRequest,
    responses(
        (status = 200, body = IndexerSearchResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn search_indexers(
    State(state): State<AppState>,
    Json(request): Json<IndexerSearchRequest>,
) -> Result<Json<IndexerSearchResponse>, ApiError> {
    let search = crate::indexer_handlers::search_input(&IndexerSearch {
        query: request.query.clone(),
        max_age_days: request.max_age_days,
        hide_passworded: request.hide_passworded,
        pretime: request.pretime,
    })?;
    let limit = request.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if !(1..=rd_subscription::MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(ApiError::unprocessable(
            "indexer.limit_invalid",
            "A search page holds 1 to 500 results",
        )
        .with_param("maximum", rd_subscription::MAX_SEARCH_LIMIT));
    }
    let offset = request.offset.unwrap_or_default().min(MAX_SEARCH_OFFSET);
    let categories = crate::subscription_handlers::sanitize_source_categories(&request.categories)?;
    let typed = typed::typed_input(&request)?;
    let indexers = chosen(&state, &request.indexer_ids).await?;

    // Every indexer at once: each answers on its own clock, and one that hangs must not hold
    // up the others' hits. The order of the outcomes stays the order they were asked in.
    let mut tasks = tokio::task::JoinSet::new();
    for (position, indexer) in indexers.into_iter().enumerate() {
        let state = state.clone();
        let search = search.clone();
        let categories = categories.clone();
        let typed = typed.clone();
        tasks.spawn(async move {
            let asked = Asked {
                search: &search,
                typed: &typed,
                categories: &categories,
                limit,
                offset,
            };
            let answer = tokio::time::timeout(SEARCH_TIMEOUT, search_one(&state, &indexer, &asked))
                .await
                .unwrap_or_else(|_| {
                    Err(ApiError::bad_gateway(
                        "indexer.unreachable",
                        "The indexer did not answer in time",
                    )
                    .with_param("reason", "timeout"))
                });
            (position, indexer, answer)
        });
    }
    let mut answers = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        answers.push(joined.map_err(anyhow::Error::new)?);
    }
    answers.sort_by_key(|(position, _, _)| *position);

    let mut response = IndexerSearchResponse {
        hits: Vec::new(),
        indexers: Vec::with_capacity(answers.len()),
    };
    for (_, indexer, answer) in answers {
        match answer {
            Ok((hits, page)) => {
                response.indexers.push(IndexerSearchOutcome {
                    indexer_id: indexer.id,
                    indexer_name: indexer.name.clone(),
                    returned: u32::try_from(hits.len()).unwrap_or(u32::MAX),
                    total: page.total,
                    more: page.announced >= usize::try_from(limit).unwrap_or(usize::MAX),
                    error: None,
                });
                response.hits.extend(hits);
            }
            Err(error) => {
                tracing::info!(
                    indexer = %indexer.name,
                    code = error.code(),
                    "indexer search failed"
                );
                response.indexers.push(IndexerSearchOutcome {
                    indexer_id: indexer.id,
                    indexer_name: indexer.name.clone(),
                    returned: 0,
                    total: None,
                    more: false,
                    error: Some(error.into_message()),
                });
            }
        }
    }
    Ok(Json(response))
}

/// The indexers a search asks: the named ones, or every enabled one.
async fn chosen(state: &AppState, ids: &[IndexerId]) -> Result<Vec<Indexer>, ApiError> {
    let all = state.database.list_indexers().await?;
    if ids.is_empty() {
        let enabled: Vec<Indexer> = all.into_iter().filter(|indexer| indexer.enabled).collect();
        if enabled.is_empty() {
            return Err(ApiError::unprocessable(
                "indexer.none_enabled",
                "There is no enabled indexer to search",
            ));
        }
        return Ok(enabled);
    }
    let mut picked = Vec::with_capacity(ids.len());
    for id in ids {
        let indexer = all
            .iter()
            .find(|indexer| indexer.id == *id)
            .ok_or_else(|| ApiError::not_found("indexer.not_found", "Indexer not found"))?;
        if !picked.iter().any(|kept: &Indexer| kept.id == *id) {
            picked.push(indexer.clone());
        }
    }
    Ok(picked)
}

/// What one search asks of every indexer it goes to.
struct Asked<'a> {
    search: &'a IndexerSearch,
    typed: &'a rd_subscription::TypedSearch,
    categories: &'a [String],
    limit: u32,
    offset: u32,
}

/// One request to one indexer, read into hits whose addresses no longer carry the key.
async fn search_one(
    state: &AppState,
    indexer: &Indexer,
    asked: &Asked<'_>,
) -> Result<(Vec<IndexerSearchHit>, rd_subscription::SearchPage), ApiError> {
    let key = crate::indexer_handlers::api_key(state, indexer).await?;
    let categories = if asked.categories.is_empty() {
        &indexer.categories
    } else {
        asked.categories
    };
    let url = rd_subscription::build_indexer_query(
        &indexer.url,
        &key,
        &rd_subscription::IndexerQuery {
            limit: asked.limit,
            offset: asked.offset,
            categories,
            search: asked.search,
            typed: Some(asked.typed),
        },
    )
    .map_err(|error| ApiError::unprocessable("indexer.url_invalid", error.to_string()))?;
    // The same client a subscription poll uses: the global proxy and TLS defaults, and the
    // person's own network allowed, because the address is one they typed themselves.
    let network = state.scheduler.direct_client(&url).await.map_err(|error| {
        ApiError::bad_gateway("indexer.unreachable", error.to_string())
            .with_param("reason", "client")
    })?;
    let fetched = rd_http::fetch_conditional(
        &network.client,
        url.clone(),
        &network.headers,
        rd_subscription::MAX_FEED_BYTES,
    )
    .await
    .map_err(|error| {
        // The address carries the key; only its redacted form may reach a message.
        ApiError::bad_gateway(
            "indexer.unreachable",
            format!("{error} ({})", rd_subscription::redact_query(&url)),
        )
        .with_param("reason", "request")
    })?;
    let body = fetched.body.unwrap_or_default();
    if let Some(refusal) = rd_subscription::indexer_refusal(&body) {
        return Err(refusal_error(&refusal, indexer));
    }
    let base = fetched.final_url;
    let page = rd_subscription::parse_search(&body, &base).map_err(|error| {
        ApiError::bad_gateway("indexer.answer_invalid", error.to_string())
            .with_param("indexer", indexer.name.clone())
    })?;
    let hits = page
        .hits
        .iter()
        .map(|hit| IndexerSearchHit {
            indexer_id: indexer.id,
            indexer_name: indexer.name.clone(),
            title: hit.title.clone(),
            guid: hit.guid.as_deref().map(|guid| without_key(guid, &key)),
            download: without_key(hit.download.as_str(), &key),
            size_bytes: hit.size_bytes,
            published_at: hit.published_at,
            category: hit.category.clone(),
            grabs: hit.grabs,
            passworded: hit.passworded,
            metadata: hit.metadata.clone(),
            cover_url: cover_without_key(hit.cover_url.as_ref(), &key),
            kind: if hit.torrent {
                IndexerHitKind::Torrent
            } else {
                IndexerHitKind::Nzb
            },
            seeders: hit.seeders,
            leechers: hit.leechers,
            magnet: hit
                .magnet
                .as_ref()
                .map(|magnet| without_key(magnet.as_str(), &key)),
        })
        .collect();
    Ok((hits, page))
}

/// An indexer's `<error code=… description=…/>`, as a code the interface translates.
///
/// Newznab's numbering: `1xx` is the account (100 wrong credentials, 101 suspended, 102 no
/// permission), `2xx` the request (201 an incorrect parameter -- a search term that is too short
/// among them), `5xx` a limit (500 requests, 501 downloads). Anything else keeps its number.
pub(crate) fn refusal_error(
    refusal: &rd_subscription::IndexerRefusal,
    indexer: &Indexer,
) -> ApiError {
    let (code, text) = match refusal.number() {
        Some(100..=199) => (
            "indexer.credentials_refused",
            "The indexer refused the API key",
        ),
        Some(200..=299) => ("indexer.query_rejected", "The indexer refused the search"),
        Some(500..=599) => (
            "indexer.limit_reached",
            "The indexer's request or download limit is reached",
        ),
        _ => ("indexer.refused", "The indexer refused the request"),
    };
    ApiError::bad_gateway(code, format!("{text}: {}", refusal.message()))
        .with_param("indexer", indexer.name.clone())
        .with_param("code", refusal.code.clone().unwrap_or_default())
        .with_param(
            "description",
            refusal.description.clone().unwrap_or_default(),
        )
}

/// The text with every occurrence of the key -- as written and as a URL encodes it -- replaced
/// by [`KEY_PLACEHOLDER`].
pub(crate) fn without_key(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_owned();
    }
    let encoded: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
    let mut scrubbed = text.replace(key, KEY_PLACEHOLDER);
    if encoded != key {
        scrubbed = scrubbed.replace(&encoded, KEY_PLACEHOLDER);
    }
    scrubbed
}

/// The cover address, unless it carries the key: a picture is fetched by the browser, so an
/// address with the key in it would hand the key to the page. Dropped rather than scrubbed --
/// with the placeholder in it the address would not load anyway, and the row shows the
/// placeholder picture for a cover it does not have.
fn cover_without_key(cover: Option<&url::Url>, key: &str) -> Option<String> {
    let cover = cover?.as_str();
    (without_key(cover, key) == cover).then(|| cover.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{KEY_PLACEHOLDER, cover_without_key, without_key};

    #[test]
    fn a_cover_that_carries_the_key_is_dropped() {
        let plain: url::Url = "https://indexer.test/covers/1.jpg".parse().expect("url");
        assert_eq!(
            cover_without_key(Some(&plain), "S3CRET").as_deref(),
            Some("https://indexer.test/covers/1.jpg")
        );
        let keyed: url::Url = "https://indexer.test/covers/1.jpg?apikey=S3CRET"
            .parse()
            .expect("url");
        assert_eq!(cover_without_key(Some(&keyed), "S3CRET"), None);
        assert_eq!(cover_without_key(None, "S3CRET"), None);
    }

    #[test]
    fn the_key_is_replaced_wherever_it_stands() {
        let scrubbed = without_key(
            "https://indexer.test/getnzb/abc.nzb?i=7&r=S3CRET&apikey=S3CRET",
            "S3CRET",
        );
        assert!(!scrubbed.contains("S3CRET"), "{scrubbed}");
        assert_eq!(scrubbed.matches(KEY_PLACEHOLDER).count(), 2);
        let path = without_key("https://indexer.test/download/S3CRET/abc", "S3CRET");
        assert_eq!(
            path,
            format!("https://indexer.test/download/{KEY_PLACEHOLDER}/abc")
        );
        // A key a URL has to encode is found in its encoded form too.
        let encoded = without_key("https://indexer.test/api?apikey=a%2Bb", "a+b");
        assert!(!encoded.contains("a%2Bb"), "{encoded}");
        assert_eq!(without_key("nothing here", ""), "nothing here");
    }
}

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
//! parser, password convention and review list a dropped `.nzb` takes.

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use rd_core::{CategoryId, Indexer, IndexerId, IndexerSearch};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, dto::MessageResponse, error::ApiError};

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

/// One search over one indexer or all enabled ones.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct IndexerSearchRequest {
    /// The indexers to ask; empty asks every enabled one.
    #[serde(default)]
    pub indexer_ids: Vec<IndexerId>,
    /// Sent as `q`: empty, or at least three characters. `!word` excludes a word, as the
    /// indexer defines it.
    #[serde(default)]
    pub query: Option<String>,
    /// The indexer's own category ids, sent as `cat`; empty uses each indexer's own default.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Sent as `maxage`: only releases posted within this many days.
    #[serde(default)]
    pub max_age_days: Option<u32>,
    /// Sent as `pw=2`: leave out releases the indexer marks as passworded.
    #[serde(default)]
    pub hide_passworded: bool,
    /// Sent as `pred` (0, 1 or 2 as the indexer defines them).
    #[serde(default)]
    pub pretime: Option<u8>,
    /// Results per indexer on this page, 1-500; 100 when absent.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Where the page starts.
    #[serde(default)]
    pub offset: Option<u32>,
}

/// One hit, as the result list shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct IndexerSearchHit {
    pub indexer_id: IndexerId,
    pub indexer_name: String,
    pub title: String,
    /// The indexer's own id for the release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// Where the NZB is fetched from, with the API key replaced by `rdownloader-indexer-key`:
    /// what `POST /api/v1/indexers/grab` takes back. Never the key itself.
    pub download: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    /// The indexer's category id, e.g. `5040`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grabs: Option<u64>,
    /// Whether the indexer marks the release as passworded.
    pub passworded: bool,
}

/// How one indexer answered.
#[derive(Serialize, ToSchema)]
pub struct IndexerSearchOutcome {
    pub indexer_id: IndexerId,
    pub indexer_name: String,
    /// Hits it returned on this page.
    pub returned: u32,
    /// The total it reports, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Whether it filled the page, so a next page may hold more.
    pub more: bool,
    /// Why it answered nothing: a stable code with its parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MessageResponse>,
}

/// A search's answer: every indexer's hits together, and how each indexer fared.
#[derive(Serialize, ToSchema)]
pub struct IndexerSearchResponse {
    pub hits: Vec<IndexerSearchHit>,
    /// One entry per indexer asked, in the order they were asked.
    pub indexers: Vec<IndexerSearchOutcome>,
}

/// Hits to fetch and put into the LinkGrabber as NZB imports.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IndexerGrabRequest {
    /// At most 50.
    pub items: Vec<IndexerGrabItem>,
    /// The category the imports go to; the routing rules decide when absent.
    #[serde(default)]
    pub category_id: Option<CategoryId>,
}

/// One hit, as the search returned it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IndexerGrabItem {
    pub indexer_id: IndexerId,
    /// The hit's `download`, unchanged.
    pub download: String,
    /// The hit's title, which names the import.
    pub title: String,
}

/// One hit that did not become an import.
#[derive(Serialize, ToSchema)]
pub struct IndexerGrabFailure {
    pub title: String,
    pub error: MessageResponse,
}

/// What a grab became.
#[derive(Serialize, ToSchema)]
pub struct IndexerGrabResponse {
    pub imports: Vec<rd_core::NzbImport>,
    pub failed: Vec<IndexerGrabFailure>,
}

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
    let indexers = chosen(&state, &request.indexer_ids).await?;

    // Every indexer at once: each answers on its own clock, and one that hangs must not hold
    // up the others' hits. The order of the outcomes stays the order they were asked in.
    let mut tasks = tokio::task::JoinSet::new();
    for (position, indexer) in indexers.into_iter().enumerate() {
        let state = state.clone();
        let search = search.clone();
        let categories = categories.clone();
        tasks.spawn(async move {
            let answer = tokio::time::timeout(
                SEARCH_TIMEOUT,
                search_one(&state, &indexer, &search, &categories, limit, offset),
            )
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

/// One request to one indexer, read into hits whose addresses no longer carry the key.
async fn search_one(
    state: &AppState,
    indexer: &Indexer,
    search: &IndexerSearch,
    categories: &[String],
    limit: u32,
    offset: u32,
) -> Result<(Vec<IndexerSearchHit>, rd_subscription::SearchPage), ApiError> {
    let key = crate::indexer_handlers::api_key(state, indexer).await?;
    let categories = if categories.is_empty() {
        &indexer.categories
    } else {
        categories
    };
    let url = rd_subscription::build_indexer_query(
        &indexer.url,
        &key,
        &rd_subscription::IndexerQuery {
            limit,
            offset,
            categories,
            search,
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

/// Fetches the chosen hits and puts each into the LinkGrabber as an NZB import (RD-180-19).
///
/// Each hit on its own: one that fails is reported with a stable code and the others still
/// arrive, because a person who picked ten releases wants the nine that worked.
#[utoipa::path(
    post,
    path = "/api/v1/indexers/grab",
    tag = "indexers",
    request_body = IndexerGrabRequest,
    responses(
        (status = 200, body = IndexerGrabResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn grab_indexer_results(
    State(state): State<AppState>,
    Json(request): Json<IndexerGrabRequest>,
) -> Result<Json<IndexerGrabResponse>, ApiError> {
    if request.items.is_empty() {
        return Err(ApiError::bad_request(
            "indexer.grab_empty",
            "Choose at least one result",
        ));
    }
    if request.items.len() > MAX_GRAB_ITEMS {
        return Err(ApiError::unprocessable(
            "indexer.grab_too_many",
            "Too many results in one request",
        )
        .with_param("maximum", MAX_GRAB_ITEMS));
    }
    if let Some(category_id) = request.category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == category_id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    let mut response = IndexerGrabResponse {
        imports: Vec::new(),
        failed: Vec::new(),
    };
    // One after the other rather than at once: the requests go to the same few servers, which
    // count them, and a burst is how a download limit is met in the middle of a selection.
    for item in request.items {
        match grab_one(&state, &item, request.category_id).await {
            Ok(import) => response.imports.push(import),
            Err(error) => {
                tracing::info!(code = error.code(), "indexer grab failed");
                response.failed.push(IndexerGrabFailure {
                    title: item.title,
                    error: error.into_message(),
                });
            }
        }
    }
    Ok(Json(response))
}

async fn grab_one(
    state: &AppState,
    item: &IndexerGrabItem,
    category_id: Option<CategoryId>,
) -> Result<rd_core::NzbImport, ApiError> {
    let indexer = crate::indexer_handlers::stored(state, item.indexer_id).await?;
    let raw = item.download.trim();
    let invalid = || {
        ApiError::bad_request(
            "indexer.download_invalid",
            "The result's address is not an http or https URL",
        )
    };
    let named = url::Url::parse(raw).map_err(|_| invalid())?;
    if !matches!(named.scheme(), "http" | "https") || named.host_str().is_none() {
        return Err(invalid());
    }
    let own_server = named.origin() == indexer.url.origin();
    let url = if raw.contains(KEY_PLACEHOLDER) {
        // The key goes to the indexer's own server and nowhere else.
        if !own_server {
            return Err(ApiError::forbidden(
                "indexer.download_foreign",
                "The result's address is not on the indexer's server",
            )
            .with_param("indexer", indexer.name.clone()));
        }
        let key = crate::indexer_handlers::api_key(state, &indexer).await?;
        let encoded: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
        url::Url::parse(&raw.replace(KEY_PLACEHOLDER, &encoded)).map_err(|_| invalid())?
    } else {
        named
    };
    let network = if own_server {
        // The indexer's own server is the person's word, like its search: their own network
        // included.
        state.scheduler.direct_client(&url).await
    } else {
        // Anything else was proposed by the indexer's answer, and is held to the rule the
        // LinkGrabber's proposed links keep to: never this machine, not the local network.
        let policy = state.scheduler.remote_address_policy(false);
        if let Err(rd_http::TargetRefusal::Refused(_)) =
            rd_http::check_target(&policy, &rd_http::SystemLookup, &url).await
        {
            return Err(ApiError::forbidden(
                "indexer.download_address_refused",
                "The result's address points at this machine or into your own network",
            ));
        }
        state.scheduler.guarded_client(&url, policy).await
    }
    .map_err(|error| {
        ApiError::bad_gateway("indexer.unreachable", error.to_string())
            .with_param("reason", "client")
    })?;
    let fetched = rd_http::fetch_document(
        &network.client,
        url.clone(),
        &network.headers,
        rd_collector::MAX_NZB_BYTES,
    )
    .await
    .map_err(|error| {
        ApiError::bad_gateway(
            "indexer.nzb_fetch_failed",
            format!("{error} ({})", rd_core::redact_url(&url)),
        )
        .with_param("indexer", indexer.name.clone())
    })?;
    // A refusal arrives inside a `200 OK`, as headers or as an error document; without the
    // check the person would be told the NZB is broken.
    if let Some(refusal) = crate::collector_enqueue::indexer_refusal(&fetched) {
        return Err(ApiError::bad_gateway("indexer.refused", refusal)
            .with_param("indexer", indexer.name.clone()));
    }
    if let Some(refusal) = std::str::from_utf8(&fetched.bytes)
        .ok()
        .and_then(rd_subscription::indexer_refusal)
    {
        return Err(refusal_error(&refusal, &indexer));
    }
    let title = item.title.trim();
    let file_name = if title.is_empty() {
        "indexer.nzb".to_owned()
    } else {
        format!("{title}.nzb")
    };
    crate::nzb_handlers::store_nzb_import(
        state,
        &fetched.bytes,
        &file_name,
        category_id,
        // The person searched and chose this release, as they would upload a file.
        rd_core::IngressSource::Manual,
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{KEY_PLACEHOLDER, without_key};

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

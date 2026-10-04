//! Newznab indexers defined once (RD-180-19): the REST surface that stores them, tests them with
//! `t=caps`, and hands one over to an indexer subscription (RD-180-20).
//!
//! The API key is written into the vault the moment it arrives and never leaves it again: no
//! response carries it, and a log line or an error names the redacted address only. Like a
//! Usenet server, an indexer edit writes no audit record, so there is no record to leak it into.
//! The search and the grab that use the key are in [`crate::indexer_search`].

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_api_core::input_checks::{TextLimit, required_text};
use rd_core::{Indexer, IndexerId, IndexerListStyle, IndexerSearch};
use rd_db::{NewIndexer, StoreErrorKind};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// Longest name and address accepted, the bounds a subscription has.
const MAX_NAME: usize = 200;
const MAX_URL: usize = 2_000;

/// Create or replace one indexer.
#[derive(Deserialize, ToSchema)]
pub struct IndexerRequest {
    pub name: String,
    /// The API base address, e.g. `https://api.example.org/api`. http or https.
    #[schema(format = "uri")]
    pub url: String,
    /// The API key; write-only, stored in the vault. Required when creating; omitted on an edit
    /// it keeps the stored key.
    #[serde(default)]
    #[schema(write_only)]
    pub api_key: Option<String>,
    /// The indexer's own category ids a search asks for when it names none. Empty asks for all.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How the LinkGrabber's search draws this indexer's hits (RD-190-16); `compact` when
    /// absent. Like every field here an edit replaces it, so a form sends the stored one back.
    #[serde(default)]
    pub list_style: IndexerListStyle,
}

const fn default_true() -> bool {
    true
}

/// Validates the request into what the store takes; the key is minted by the caller.
fn indexer_input(
    request: &IndexerRequest,
    secret_ref: Option<String>,
) -> Result<NewIndexer, ApiError> {
    let name = required_text(
        &request.name,
        TextLimit::Chars(MAX_NAME),
        "indexer.name_invalid",
        "An indexer needs a name",
    )?;
    Ok(NewIndexer {
        name,
        url: indexer_url(&request.url)?,
        secret_ref,
        categories: crate::subscription_handlers::sanitize_source_categories(&request.categories)?,
        enabled: request.enabled,
        list_style: request.list_style,
    })
}

/// An indexer's address: http or https, nothing else.
///
/// The same rule a subscription's address follows. The person typed it, so it may be on their
/// own network -- an NZBHydra or Prowlarr on the NAS is the common setup.
pub(crate) fn indexer_url(raw: &str) -> Result<url::Url, ApiError> {
    let raw = required_text(
        raw,
        TextLimit::Bytes(MAX_URL),
        "indexer.url_invalid",
        "An indexer needs an address",
    )?;
    let url = url::Url::parse(&raw)
        .map_err(|_| ApiError::bad_request("indexer.url_invalid", "Address is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ApiError::bad_request(
            "indexer.url_scheme",
            "Only http and https addresses can be searched",
        ));
    }
    Ok(url)
}

/// Checks and cleans the search parameters, for an interactive search and a subscription alike.
///
/// `q` is empty or at least three characters: a Newznab server answers a shorter one with error
/// 201, and refusing it here says so in the person's language before a request is spent.
pub(crate) fn search_input(search: &IndexerSearch) -> Result<IndexerSearch, ApiError> {
    let query = search
        .query
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty());
    if let Some(term) = query {
        let length = term.chars().count();
        if length < rd_core::MIN_INDEXER_QUERY_CHARS {
            return Err(ApiError::unprocessable(
                "indexer.query_too_short",
                "A search term needs at least three characters",
            )
            .with_param("minimum", rd_core::MIN_INDEXER_QUERY_CHARS));
        }
        if length > rd_core::MAX_INDEXER_QUERY_CHARS {
            return Err(ApiError::unprocessable(
                "indexer.query_too_long",
                "The search term is too long",
            )
            .with_param("maximum", rd_core::MAX_INDEXER_QUERY_CHARS));
        }
        if term.contains(['\0', '\n', '\r']) {
            return Err(ApiError::unprocessable(
                "indexer.query_invalid",
                "A search term may not contain a line break",
            ));
        }
    }
    if let Some(days) = search.max_age_days
        && !(1..=rd_core::MAX_INDEXER_AGE_DAYS).contains(&days)
    {
        return Err(ApiError::unprocessable(
            "indexer.max_age_invalid",
            "The maximum age is outside the permitted range",
        )
        .with_param("maximum", rd_core::MAX_INDEXER_AGE_DAYS));
    }
    if let Some(pretime) = search.pretime
        && pretime > rd_core::MAX_INDEXER_PRETIME
    {
        return Err(ApiError::unprocessable(
            "indexer.pretime_invalid",
            "pred takes 0, 1 or 2",
        ));
    }
    Ok(IndexerSearch {
        query: query.map(ToOwned::to_owned),
        ..search.clone()
    })
}

fn not_found() -> ApiError {
    ApiError::not_found("indexer.not_found", "Indexer not found")
}

/// Maps the store's refusals onto coded answers.
fn store_error(error: anyhow::Error) -> ApiError {
    match rd_db::store_kind(&error) {
        Some(StoreErrorKind::NotFound) => not_found(),
        Some(StoreErrorKind::Duplicate) => ApiError::conflict(
            "indexer.name_taken",
            "An indexer with this name already exists",
        ),
        _ => error.into(),
    }
}

/// The stored indexer, or a coded 404.
pub(crate) async fn stored(state: &AppState, id: IndexerId) -> Result<Indexer, ApiError> {
    state.database.indexer(id).await?.ok_or_else(not_found)
}

/// The indexer's key, out of the vault.
pub(crate) async fn api_key(state: &AppState, indexer: &Indexer) -> Result<String, ApiError> {
    let missing = || {
        ApiError::unprocessable("indexer.api_key_missing", "That indexer has no API key")
            .with_param("indexer", indexer.name.clone())
    };
    let reference = indexer.secret_ref.as_deref().ok_or_else(missing)?;
    let key = state.secrets.get(reference).await.map_err(|_| missing())?;
    Ok(secrecy::ExposeSecret::expose_secret(&key).to_owned())
}

#[utoipa::path(
    get,
    path = "/api/v1/indexers",
    tag = "indexers",
    responses((status = 200, body = Vec<Indexer>))
)]
pub async fn list_indexers(State(state): State<AppState>) -> Result<Json<Vec<Indexer>>, ApiError> {
    Ok(Json(state.database.list_indexers().await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/indexers",
    tag = "indexers",
    request_body = IndexerRequest,
    responses(
        (status = 201, body = Indexer),
        (status = 400, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn create_indexer(
    State(state): State<AppState>,
    Json(request): Json<IndexerRequest>,
) -> Result<(StatusCode, Json<Indexer>), ApiError> {
    // Validated before the key is minted, so a refused request leaves nothing in the vault.
    indexer_input(&request, None)?;
    if request
        .api_key
        .as_deref()
        .is_none_or(|key| key.trim().is_empty())
    {
        return Err(ApiError::unprocessable(
            "indexer.api_key_missing",
            "An indexer needs an API key",
        ));
    }
    let secret_ref = crate::config_fields::store_optional(
        &state.secrets,
        request.api_key.as_deref().map(|key| key.trim().to_owned()),
    )
    .await?;
    let input = indexer_input(&request, secret_ref.clone())?;
    match state.database.create_indexer(input).await {
        Ok(created) => Ok((StatusCode::CREATED, Json(created))),
        Err(error) => {
            // A reference nothing points at would never be cleaned up.
            crate::config_fields::cleanup_secrets(&state.secrets, [secret_ref]).await;
            Err(store_error(error))
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/indexers/{id}",
    tag = "indexers",
    params(("id" = IndexerId, Path)),
    request_body = IndexerRequest,
    responses(
        (status = 200, body = Indexer),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn update_indexer(
    State(state): State<AppState>,
    Path(id): Path<IndexerId>,
    Json(request): Json<IndexerRequest>,
) -> Result<Json<Indexer>, ApiError> {
    indexer_input(&request, None)?;
    let minted = crate::config_fields::store_optional(
        &state.secrets,
        request.api_key.as_deref().map(|key| key.trim().to_owned()),
    )
    .await?;
    let input = indexer_input(&request, minted.clone())?;
    match state.database.update_indexer(id, input).await {
        Ok((updated, orphan)) => {
            // Only the key this edit replaced goes; an unchanged one survives a form that did
            // not resend it.
            crate::config_fields::cleanup_secrets(&state.secrets, [orphan]).await;
            Ok(Json(updated))
        }
        Err(error) => {
            crate::config_fields::cleanup_secrets(&state.secrets, [minted]).await;
            Err(store_error(error))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/indexers/{id}",
    tag = "indexers",
    params(("id" = IndexerId, Path)),
    responses((status = 204), (status = 404, body = crate::error::ErrorBody))
)]
pub async fn delete_indexer(
    State(state): State<AppState>,
    Path(id): Path<IndexerId>,
) -> Result<StatusCode, ApiError> {
    let secret_ref = state
        .database
        .delete_indexer(id)
        .await
        .map_err(store_error)?;
    crate::config_fields::cleanup_secrets(&state.secrets, [secret_ref]).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Tests a stored indexer: `t=caps` with its address and key (RD-180-19).
///
/// The cheapest request that proves both are right, and where the category list for the form
/// comes from. A new indexer, before it has a stored key, is tested through
/// `POST /api/v1/subscriptions/caps`, which takes the address and the key for one request.
#[utoipa::path(
    post,
    path = "/api/v1/indexers/{id}/caps",
    tag = "indexers",
    params(("id" = IndexerId, Path)),
    responses(
        (status = 200, body = rd_subscription::IndexerCaps),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn indexer_caps(
    State(state): State<AppState>,
    Path(id): Path<IndexerId>,
) -> Result<Json<rd_subscription::IndexerCaps>, ApiError> {
    let indexer = stored(&state, id).await?;
    let key = api_key(&state, &indexer).await?;
    crate::subscription_handlers::fetch_caps(&state, &indexer.url, &key)
        .await
        .map(Json)
}

/// Takes a defined indexer over into a subscription request (RD-180-20).
///
/// A copy, not a link: the subscription gets the indexer's address -- unless it names a saved
/// search on the same server -- and a vault entry of its own holding the indexer's key, unless
/// the request brings one. The poller stays as it was, and deleting or editing the indexer later
/// changes no subscription; one that should follow a new key takes the indexer over again.
///
/// The address a request names has to be the indexer's own server (scheme, host and port):
/// otherwise this would be a way to send a stored key to any address.
pub(crate) async fn take_over(
    state: &AppState,
    request: &mut crate::subscription_handlers::SubscriptionRequest,
) -> Result<(), ApiError> {
    let Some(id) = request.indexer_id else {
        return Ok(());
    };
    if request.kind != rd_core::SubscriptionKind::Indexer {
        return Err(ApiError::unprocessable(
            "subscription.indexer_kind",
            "Only an indexer subscription takes over a defined indexer",
        ));
    }
    let indexer = stored(state, id).await?;
    if request.url.trim().is_empty() {
        request.url = indexer.url.to_string();
    } else {
        let named = url::Url::parse(request.url.trim()).map_err(|_| {
            ApiError::bad_request("subscription.url_invalid", "Address is not a URL")
        })?;
        if named.origin() != indexer.url.origin() {
            return Err(ApiError::unprocessable(
                "subscription.indexer_url_mismatch",
                "The address is not on the chosen indexer's server",
            )
            .with_param("indexer", indexer.name.clone()));
        }
    }
    if request
        .api_key
        .as_deref()
        .is_none_or(|key| key.trim().is_empty())
    {
        request.api_key = Some(api_key(state, &indexer).await?);
    }
    if request.source_categories.is_empty() {
        request.source_categories.clone_from(&indexer.categories);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rd_core::IndexerSearch;

    use super::{indexer_url, search_input};

    #[test]
    fn a_search_term_is_empty_or_three_characters_or_more() {
        for term in ["ab", " x ", "!a"] {
            let search = IndexerSearch {
                query: Some(term.to_owned()),
                ..IndexerSearch::default()
            };
            assert_eq!(
                search_input(&search).expect_err(term).code(),
                "indexer.query_too_short",
                "{term:?}"
            );
        }
        let blank = IndexerSearch {
            query: Some("   ".to_owned()),
            ..IndexerSearch::default()
        };
        assert_eq!(search_input(&blank).expect("blank").query, None);
        let kept = IndexerSearch {
            query: Some("  some show !cam ".to_owned()),
            ..IndexerSearch::default()
        };
        assert_eq!(
            search_input(&kept).expect("kept").query.as_deref(),
            Some("some show !cam")
        );
    }

    #[test]
    fn the_age_and_the_pretime_stay_in_their_ranges() {
        let zero_days = IndexerSearch {
            max_age_days: Some(0),
            ..IndexerSearch::default()
        };
        assert_eq!(
            search_input(&zero_days).expect_err("zero").code(),
            "indexer.max_age_invalid"
        );
        let pretime = IndexerSearch {
            pretime: Some(3),
            ..IndexerSearch::default()
        };
        assert_eq!(
            search_input(&pretime).expect_err("three").code(),
            "indexer.pretime_invalid"
        );
        let fine = IndexerSearch {
            max_age_days: Some(30),
            pretime: Some(2),
            hide_passworded: true,
            ..IndexerSearch::default()
        };
        assert_eq!(search_input(&fine).expect("fine"), fine);
    }

    #[test]
    fn an_indexer_address_is_http_or_https() {
        assert!(indexer_url("https://api.example.test/api").is_ok());
        assert!(indexer_url("http://192.168.1.10:5076/api").is_ok());
        assert_eq!(
            indexer_url("ftp://example.test/").expect_err("ftp").code(),
            "indexer.url_scheme"
        );
        assert_eq!(
            indexer_url("   ").expect_err("empty").code(),
            "indexer.url_invalid"
        );
    }
}

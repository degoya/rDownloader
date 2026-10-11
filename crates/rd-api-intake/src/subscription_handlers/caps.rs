//! What an indexer can do, asked of a stored subscription or of one being entered (RD-080-11).

use super::*;

/// Asks an indexer what it can do, and thereby tests it (RD-080-11).
///
/// `t=caps` is the cheapest request that proves the address and the API key are both right,
/// so this is the test action as well as where the category list for the mapping UI comes
/// from. It pulls no results, so testing an indexer costs it almost nothing.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/caps",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = rd_subscription::IndexerCaps),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn subscription_caps(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<rd_subscription::IndexerCaps>, ApiError> {
    let subscription =
        state.database.subscription(id).await?.ok_or_else(|| {
            ApiError::not_found("subscription.not_found", "Subscription not found")
        })?;
    if subscription.kind != SubscriptionKind::Indexer {
        return Err(ApiError::unprocessable(
            "subscription.not_an_indexer",
            "Only an indexer subscription has capabilities",
        ));
    }
    let Some(reference) = subscription.secret_ref.as_deref() else {
        return Err(ApiError::unprocessable(
            "subscription.api_key_missing",
            "That indexer has no API key",
        ));
    };
    let api_key = state.secrets.get(reference).await.map_err(|error| {
        crate::error_codes::unless_secret_unreadable(&error, || {
            ApiError::unprocessable("subscription.api_key_missing", "Key unavailable")
        })
    })?;
    fetch_caps(
        &state,
        &subscription.url,
        secrecy::ExposeSecret::expose_secret(&api_key),
    )
    .await
    .map(Json)
}

/// Body of `POST /api/v1/subscriptions/caps`.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CapsProbeRequest {
    /// The indexer's address, as it would be stored on the subscription.
    pub url: String,
    /// The key to ask with. Used for this one request and then dropped: nothing is stored, and
    /// a subscription only gains a key when it is saved.
    #[schema(write_only)]
    pub api_key: String,
}

/// Asks an indexer what it can do, before there is a subscription to ask for.
///
/// The `{id}` route above needs a stored subscription, because it resolves the key out of the
/// vault — so categories could only be mapped by saving the indexer first and reopening it,
/// which is not how anybody expects a form to behave. The request carries the address and the
/// key instead, and the chain underneath is the same one.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/caps",
    tag = "subscriptions",
    request_body = CapsProbeRequest,
    responses(
        (status = 200, body = rd_subscription::IndexerCaps),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn probe_caps(
    State(state): State<AppState>,
    Json(request): Json<CapsProbeRequest>,
) -> Result<Json<rd_subscription::IndexerCaps>, ApiError> {
    let url: url::Url = request
        .url
        .trim()
        .parse()
        .map_err(|error: url::ParseError| {
            ApiError::unprocessable("subscription.url_invalid", error.to_string())
                .with_param("reason", error)
        })?;
    if request.api_key.trim().is_empty() {
        return Err(ApiError::unprocessable(
            "subscription.api_key_missing",
            "That indexer has no API key",
        ));
    }
    fetch_caps(&state, &url, request.api_key.trim())
        .await
        .map(Json)
}

/// The shared half: build the `t=caps` address, fetch it and parse the answer.
///
/// Every message about a failure carries the redacted address only — the real one has the key
/// in its query, and this is the one place both routes could leak it from.
pub(crate) async fn fetch_caps(
    state: &AppState,
    base: &url::Url,
    api_key: &str,
) -> Result<rd_subscription::IndexerCaps, ApiError> {
    let url = rd_subscription::build_caps_query(base, api_key).map_err(|error| {
        ApiError::unprocessable("subscription.url_invalid", error.to_string())
            .with_param("reason", error)
    })?;

    let network = state.scheduler.direct_client(&url).await.map_err(|error| {
        ApiError::bad_gateway("subscription.client_unavailable", error.to_string())
            .with_param("reason", error)
    })?;
    let body = rd_http::fetch_conditional(
        &network.client,
        url.clone(),
        &network.headers,
        rd_subscription::MAX_CAPS_BYTES,
    )
    .await
    .map_err(|error| {
        let reason = format!("{error} ({})", rd_subscription::redact_query(&url));
        ApiError::bad_gateway("subscription.caps_failed", reason.clone())
            .with_param("reason", reason)
    })?;
    let body = body.body.unwrap_or_default();
    rd_subscription::parse_caps(&body).map_err(|error| {
        ApiError::bad_gateway("subscription.caps_invalid", error.to_string())
            .with_param("reason", error)
    })
}

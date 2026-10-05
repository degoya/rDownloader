//! REST surface for subscriptions (RD-080-07).
//!
//! Validation happens here rather than in the store, following the `stream_handlers`
//! precedent: the database is handed an already-clean `NewSubscription`, and every refusal
//! carries a stable code the UI translates.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_api_core::input_checks::{TextLimit, required_text};
use rd_core::{
    BacklogPolicy, DownloadPriority, MAX_FILTER_PATTERNS, MAX_POLL_INTERVAL_SECONDS, Subscription,
    SubscriptionBulkStateResponse, SubscriptionFilters, SubscriptionHistoryClearResponse,
    SubscriptionId, SubscriptionItemId, SubscriptionItemPage, SubscriptionItemState,
    SubscriptionKind, SubscriptionMode, SubscriptionReviewSummary, SubscriptionRun,
};
use rd_db::NewSubscription;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, error::ApiError};

mod caps;
mod input;
mod items;
mod request;
mod scripts;

pub(crate) use caps::fetch_caps;
pub use caps::*;
pub(crate) use input::{sanitize_source_categories, subscription_input};
pub use items::*;
pub use request::*;
use scripts::*;

/// Longest name and URL accepted.
const MAX_NAME: usize = 200;
const MAX_URL: usize = 2_000;
/// Most items and runs one response returns.
const PAGE_LIMIT: i64 = 200;
const DEFAULT_ITEM_PAGE_LIMIT: i64 = 50;

/// How many rows a listing returns.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PageQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

impl PageQuery {
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(PAGE_LIMIT).clamp(1, PAGE_LIMIT)
    }
}

/// Server-side item archive filter.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionItemFilter {
    #[default]
    Pending,
    Queued,
    Dismissed,
    Skipped,
    All,
}

/// Pagination and state selection for the item archive.
#[derive(Debug, Deserialize, IntoParams)]
pub struct SubscriptionItemPageQuery {
    #[serde(default)]
    pub state: SubscriptionItemFilter,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

impl SubscriptionItemPageQuery {
    fn state(&self) -> Option<SubscriptionItemState> {
        match self.state {
            SubscriptionItemFilter::Pending => Some(SubscriptionItemState::Pending),
            SubscriptionItemFilter::Queued => Some(SubscriptionItemState::Queued),
            SubscriptionItemFilter::Dismissed => Some(SubscriptionItemState::Dismissed),
            SubscriptionItemFilter::Skipped => Some(SubscriptionItemState::Skipped),
            SubscriptionItemFilter::All => None,
        }
    }

    fn limit(&self) -> i64 {
        self.limit
            .unwrap_or(DEFAULT_ITEM_PAGE_LIMIT)
            .clamp(1, PAGE_LIMIT)
    }

    fn offset(&self) -> i64 {
        self.offset.unwrap_or_default().max(0)
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/subscriptions",
    tag = "subscriptions",
    responses((status = 200, body = Vec<Subscription>))
)]
pub async fn list_subscriptions(
    State(state): State<AppState>,
) -> Result<Json<Vec<Subscription>>, ApiError> {
    Ok(Json(state.database.list_subscriptions().await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/subscriptions",
    tag = "subscriptions",
    request_body = SubscriptionRequest,
    responses(
        (status = 201, body = Subscription),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn create_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    audit: crate::audit::AuditContext,
    Json(mut request): Json<SubscriptionRequest>,
) -> Result<(StatusCode, Json<Subscription>), ApiError> {
    require_admin_for_script(
        granted.as_ref().map(|axum::Extension(granted)| granted),
        &[request.kind],
    )?;
    crate::indexer_handlers::take_over(&state, &mut request).await?;
    // Validated before the key is minted, so a rejected request leaves nothing behind.
    ensure_script_exists(&state, &subscription_input(&request, None)?).await?;
    let secret_ref =
        crate::config_fields::store_optional(&state.secrets, request.api_key.clone()).await?;
    let input = subscription_input(&request, secret_ref.clone())?;
    match state.database.create_subscription(input).await {
        Ok(created) => {
            let involved = created.kind == SubscriptionKind::Script;
            audit_script_change(&state, &audit, &created, "created", involved).await;
            Ok((StatusCode::CREATED, Json(created)))
        }
        Err(error) => {
            // A reference nothing points at would never be cleaned up.
            crate::config_fields::cleanup_secrets(&state.secrets, [secret_ref]).await;
            Err(error.into())
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/subscriptions/{id}",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    request_body = SubscriptionRequest,
    responses(
        (status = 200, body = Subscription),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn update_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    audit: crate::audit::AuditContext,
    Path(id): Path<SubscriptionId>,
    Json(mut request): Json<SubscriptionRequest>,
) -> Result<Json<Subscription>, ApiError> {
    // The stored kind counts too: turning a script into a feed changes a script subscription.
    let stored = state
        .database
        .subscription(id)
        .await?
        .map(|subscription| subscription.kind);
    require_admin_for_script(
        granted.as_ref().map(|axum::Extension(granted)| granted),
        &[Some(request.kind), stored]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
    )?;
    crate::indexer_handlers::take_over(&state, &mut request).await?;
    ensure_script_exists(&state, &subscription_input(&request, None)?).await?;
    let minted =
        crate::config_fields::store_optional(&state.secrets, request.api_key.clone()).await?;
    let input = subscription_input(&request, minted.clone())?;
    match state.database.update_subscription(id, input).await {
        Ok((updated, orphan)) => {
            // Only the reference this edit replaced is dropped; an unchanged key survives a
            // form that did not resend it.
            crate::config_fields::cleanup_secrets(&state.secrets, [orphan]).await;
            let involved = updated.kind == SubscriptionKind::Script
                || stored == Some(SubscriptionKind::Script);
            audit_script_change(&state, &audit, &updated, "updated", involved).await;
            Ok(Json(updated))
        }
        Err(error) => {
            crate::config_fields::cleanup_secrets(&state.secrets, [minted]).await;
            Err(not_found(error))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/enable",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = Subscription),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn enable_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<Subscription>, ApiError> {
    require_admin_for_stored_script(
        &state,
        granted.as_ref().map(|axum::Extension(granted)| granted),
        id,
    )
    .await?;
    state
        .database
        .set_subscription_enabled(id, true)
        .await
        .map(Json)
        .map_err(not_found)
}

#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/disable",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = Subscription),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn disable_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<Subscription>, ApiError> {
    require_admin_for_stored_script(
        &state,
        granted.as_ref().map(|axum::Extension(granted)| granted),
        id,
    )
    .await?;
    state
        .database
        .set_subscription_enabled(id, false)
        .await
        .map(Json)
        .map_err(not_found)
}

#[utoipa::path(
    delete,
    path = "/api/v1/subscriptions/{id}",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses((status = 204), (status = 404, body = crate::error::ErrorBody))
)]
pub async fn delete_subscription(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
) -> Result<StatusCode, ApiError> {
    let secret_ref = state
        .database
        .delete_subscription(id)
        .await
        .map_err(not_found)?;
    crate::config_fields::cleanup_secrets(&state.secrets, [secret_ref]).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Lists one state-filtered page of a subscription's items.
#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/{id}/items/page",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path), SubscriptionItemPageQuery),
    responses(
        (status = 200, body = SubscriptionItemPage),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn list_subscription_item_page(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Query(page): Query<SubscriptionItemPageQuery>,
) -> Result<Json<SubscriptionItemPage>, ApiError> {
    if state.database.subscription(id).await?.is_none() {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    }
    Ok(Json(
        state
            .database
            .subscription_item_page(id, page.state(), page.limit(), page.offset())
            .await?,
    ))
}

/// Pending review counts for all configured indexer subscriptions.
#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/review-summary",
    tag = "subscriptions",
    responses((status = 200, body = SubscriptionReviewSummary))
)]
pub async fn subscription_review_summary(
    State(state): State<AppState>,
) -> Result<Json<SubscriptionReviewSummary>, ApiError> {
    Ok(Json(state.database.subscription_review_summary().await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/{id}/runs",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path), PageQuery),
    responses((status = 200, body = Vec<SubscriptionRun>))
)]
pub async fn list_subscription_runs(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<SubscriptionRun>>, ApiError> {
    Ok(Json(
        state.database.subscription_runs(id, page.limit()).await?,
    ))
}

/// Polls one subscription immediately, outside its schedule.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/poll",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 202),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn poll_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
) -> Result<StatusCode, ApiError> {
    let Some(subscription) = state.database.subscription(id).await? else {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    };
    // "Check now" on a script subscription runs the script: the route costs `api:queue`, the
    // script costs `api:admin` (RD-130-19).
    require_admin_for_script(
        granted.as_ref().map(|axum::Extension(granted)| granted),
        &[subscription.kind],
    )?;
    // Spawned rather than awaited: a poll contacts somebody else's server and can take the
    // configured timeout, which is far longer than a request should hold a connection.
    let service = state.subscriptions.clone();
    tokio::spawn(async move {
        if let Err(error) = service.poll_now(id).await {
            tracing::warn!(%error, "manual subscription poll failed");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

/// Maps the store's "not found" bail onto a coded 404.
fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(&error, "subscription.not_found", "Subscription not found")
}

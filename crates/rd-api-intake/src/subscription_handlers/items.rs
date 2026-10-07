//! The review of a subscription's items: their state, the history and queueing one.

use super::*;

/// Body of the item-state change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionItemStateRequest {
    pub state: SubscriptionItemState,
}

/// Sets every item that was pending when this request started.
#[utoipa::path(
    put,
    path = "/api/v1/subscriptions/{id}/items/pending",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    request_body = SubscriptionItemStateRequest,
    responses(
        (status = 200, body = SubscriptionBulkStateResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn set_pending_subscription_items_state(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Json(request): Json<SubscriptionItemStateRequest>,
) -> Result<Json<SubscriptionBulkStateResponse>, ApiError> {
    if state.database.subscription(id).await?.is_none() {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    }
    if !matches!(
        request.state,
        SubscriptionItemState::Queued | SubscriptionItemState::Dismissed
    ) {
        return Err(ApiError::unprocessable(
            "subscription.bulk_state_invalid",
            "Pending items can only be queued or dismissed",
        ));
    }
    let ids = state.database.pending_subscription_item_ids(id).await?;
    let matched = u64::try_from(ids.len()).unwrap_or(u64::MAX);
    let updated = if request.state == SubscriptionItemState::Dismissed {
        state
            .database
            .set_pending_subscription_items_state(ids, request.state)
            .await?
    } else {
        let mut ready = Vec::with_capacity(ids.len());
        for item_id in ids {
            if queue_reviewed_item(&state, item_id).await.is_ok() {
                ready.push(item_id);
            }
        }
        state
            .database
            .set_pending_subscription_items_state(ready, request.state)
            .await?
    };
    Ok(Json(SubscriptionBulkStateResponse {
        matched,
        updated,
        failed: matched.saturating_sub(updated),
    }))
}

/// Removes settled items and poll history while preserving everything still awaiting review.
#[utoipa::path(
    delete,
    path = "/api/v1/subscriptions/{id}/history",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = SubscriptionHistoryClearResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn clear_subscription_history(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<SubscriptionHistoryClearResponse>, ApiError> {
    state
        .database
        .clear_subscription_history(id)
        .await
        .map(Json)
        .map_err(not_found)
}

/// Queues or dismisses one reviewed item.
///
/// Queueing hands the item to the LinkGrabber on the way, through the same intake the automatic
/// poll uses. The state alone would only move a row in the subscription's own table, which is
/// invisible everywhere else in the application.
#[utoipa::path(
    put,
    path = "/api/v1/subscriptions/items/{id}",
    tag = "subscriptions",
    params(("id" = SubscriptionItemId, Path)),
    request_body = SubscriptionItemStateRequest,
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn set_subscription_item_state(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionItemId>,
    Json(request): Json<SubscriptionItemStateRequest>,
) -> Result<StatusCode, ApiError> {
    if request.state == rd_core::SubscriptionItemState::Queued {
        queue_reviewed_item(&state, id).await?;
    }
    state
        .database
        .set_subscription_item_state(id, request.state)
        .await
        .map_err(not_found)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Hands one reviewed item to the intake before its state is written.
///
/// Intake first, state second: an item that could not be handed over keeps its pending state,
/// so the review list still shows it and the action can be repeated. The reverse order would
/// leave a row claiming to be queued with nothing behind it.
async fn queue_reviewed_item(state: &AppState, id: SubscriptionItemId) -> Result<(), ApiError> {
    let (item, subscription) = item_with_subscription(state, id).await?;
    let url = item_download_address(state, &subscription, &item).await?;
    hand_item_to_intake(state, &subscription, &item, url).await
}

/// One item and the subscription it belongs to, each a coded 404 when it is gone.
pub(super) async fn item_with_subscription(
    state: &AppState,
    id: SubscriptionItemId,
) -> Result<(rd_core::SubscriptionItem, Subscription), ApiError> {
    let item = state
        .database
        .subscription_item(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::not_found("subscription.item_not_found", "Subscription item not found")
        })?;
    let subscription = state
        .database
        .subscription(item.subscription_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("subscription.not_found", "Subscription not found"))?;
    Ok((item, subscription))
}

/// The address the item is fetched from now.
///
/// A private repository's file is resolved with the token at the moment it is handed over; the
/// address that answers without one is valid for minutes (RD-190-13).
pub(super) async fn item_download_address(
    state: &AppState,
    subscription: &Subscription,
    item: &rd_core::SubscriptionItem,
) -> Result<url::Url, ApiError> {
    state
        .subscriptions
        .download_address(subscription, &item.url)
        .await
        .map_err(|error| {
            ApiError::bad_gateway(
                "subscription.download_address_unavailable",
                rd_core::redact_text(&error.to_string()),
            )
        })
}

/// The one way an item reaches the LinkGrabber after review: the subscription's category, the
/// intake's routing and naming rules, and the declared name, password and attributes.
pub(super) async fn hand_item_to_intake(
    state: &AppState,
    subscription: &Subscription,
    item: &rd_core::SubscriptionItem,
    url: url::Url,
) -> Result<(), ApiError> {
    let category_id = subscription.category_for(item.source_category.as_deref());
    let intake = crate::subscription_service::SubscriptionIntake {
        database: &state.database,
        link_check: &state.link_check,
        media_settings: &state.media_settings,
        gallery_settings: &state.gallery_settings,
    };
    crate::subscription_service::hand_urls_to_intake(
        &intake,
        &subscription.name,
        vec![crate::collector_intake::DeclaredLink {
            url,
            media_type: item.media_type.clone(),
            name: Some(rd_files::strip_password_marker(&item.title).0),
            // Read back from the archived row, so a hit queued after review carries the same
            // password a hit queued automatically would.
            password: item.password.clone(),
            // And the same declared attributes, through the same gate: a hit queued after
            // review must reach an enricher with exactly what an auto-queued one reaches it
            // with (RD-107-02).
            attributes: rd_subscription::retain_attributes(&item.attributes, None).attributes,
        }],
        category_id,
    )
    .await
}

//! Queueing subscription items again (RD-1150-04): a hit dismissed by mistake, or one whose
//! download is gone, goes the way its first queueing went.

use super::items::{hand_item_to_intake, item_download_address, item_with_subscription};
use super::*;

/// The most items one request queues again: a page of the archive is 50, one read 200.
const MAX_REQUEUE_ITEMS: usize = 200;

/// Body of the re-queue.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionRequeueRequest {
    /// The items to queue again, all of the subscription in the path (1-200).
    pub item_ids: Vec<SubscriptionItemId>,
    /// Queue an item whose address is still in the LinkGrabber or the download list anyway.
    /// Without it such an item is refused with `subscription.item_duplicate`, so nothing is
    /// doubled silently; with it the LinkGrabber marks the new link as a duplicate.
    #[serde(default)]
    pub allow_duplicate: bool,
}

/// One item that was not queued again, and why.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SubscriptionRequeueRefusal {
    pub item_id: SubscriptionItemId,
    /// A stable code: `subscription.item_not_found`, `subscription.item_no_source`,
    /// `subscription.item_duplicate`, or the one the hand-over failed with.
    pub code: String,
    pub message: String,
}

/// What a re-queue did: the items handed to the LinkGrabber, and the rest with their reasons.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SubscriptionRequeueResponse {
    pub requeued: Vec<SubscriptionItemId>,
    pub refused: Vec<SubscriptionRequeueRefusal>,
}

/// Queues items of one subscription again, whatever was decided about them before.
///
/// The same path as the first queueing — the subscription's category, the intake's routing and
/// naming rules, the declared name, password and attributes — and the item becomes `queued`.
/// An item without an address to fetch is refused, and one whose address is still in the
/// LinkGrabber or the download list is refused unless `allow_duplicate` says otherwise. Every
/// item stands alone: one refusal does not stop the rest, and each queued one is audited.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/items/requeue",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    request_body = SubscriptionRequeueRequest,
    responses(
        (status = 200, body = SubscriptionRequeueResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn requeue_subscription_items(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<SubscriptionId>,
    Json(request): Json<SubscriptionRequeueRequest>,
) -> Result<Json<SubscriptionRequeueResponse>, ApiError> {
    if state.database.subscription(id).await?.is_none() {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    }
    if request.item_ids.is_empty() || request.item_ids.len() > MAX_REQUEUE_ITEMS {
        return Err(crate::error_codes::bulk_range(MAX_REQUEUE_ITEMS));
    }
    let mut answer = SubscriptionRequeueResponse {
        requeued: Vec::new(),
        refused: Vec::new(),
    };
    for item_id in super::input::distinct(&request.item_ids) {
        match requeue_one(&state, &audit, id, item_id, request.allow_duplicate).await {
            Ok(()) => answer.requeued.push(item_id),
            Err(error) => answer.refused.push(SubscriptionRequeueRefusal {
                item_id,
                code: error.code().to_owned(),
                message: error.message().to_owned(),
            }),
        }
    }
    Ok(Json(answer))
}

/// Intake first, state second, as in the first queueing: a refused item keeps its state.
async fn requeue_one(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    subscription_id: SubscriptionId,
    item_id: SubscriptionItemId,
    allow_duplicate: bool,
) -> Result<(), ApiError> {
    let (item, subscription) = item_with_subscription(state, item_id).await?;
    if subscription.id != subscription_id {
        return Err(ApiError::not_found(
            "subscription.item_not_found",
            "Subscription item not found",
        ));
    }
    if !has_source(&item.url) {
        return Err(ApiError::unprocessable(
            "subscription.item_no_source",
            "This item has no address to fetch",
        ));
    }
    let url = item_download_address(state, &subscription, &item).await?;
    let duplicate = address_taken(state, &url, &item.url).await?;
    if duplicate && !allow_duplicate {
        return Err(ApiError::conflict(
            "subscription.item_duplicate",
            "This item's address is still in the LinkGrabber or the download list",
        ));
    }
    hand_item_to_intake(state, &subscription, &item, url).await?;
    state
        .database
        .set_subscription_item_state(item_id, SubscriptionItemState::Queued)
        .await
        .map_err(not_found)?;
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SubscriptionItemRequeued)
            .by(audit)
            .target("subscription_item", item_id)
            .named(rd_files::strip_password_marker(&item.title).0)
            .detail("subscription", subscription.id)
            .detail("previous_state", item_state_word(item.state))
            .detail("duplicate", duplicate),
    )
    .await;
    Ok(())
}

/// Whether the item carries an address something can be fetched from: a host, or a magnet
/// link. The web UI applies the same rule to switch the action off (`hitHasSource`).
fn has_source(url: &url::Url) -> bool {
    url.scheme() == "magnet" || url.host_str().is_some_and(|host| !host.is_empty())
}

/// The intake's duplicate test, for the address handed over and, where the subscription
/// resolves a fresh one each time, for the stored one too.
async fn address_taken(
    state: &AppState,
    url: &url::Url,
    stored: &url::Url,
) -> Result<bool, ApiError> {
    if state.database.address_in_collector_or_queue(url).await? {
        return Ok(true);
    }
    Ok(stored != url && state.database.address_in_collector_or_queue(stored).await?)
}

fn item_state_word(item_state: SubscriptionItemState) -> &'static str {
    match item_state {
        SubscriptionItemState::Pending => "pending",
        SubscriptionItemState::Queued => "queued",
        SubscriptionItemState::Skipped => "skipped",
        SubscriptionItemState::Dismissed => "dismissed",
    }
}

#[cfg(test)]
mod tests {
    use super::has_source;

    #[test]
    fn an_address_with_a_host_or_a_magnet_link_is_a_source_and_nothing_else() {
        for source in [
            "https://indexer.test/api?t=get&id=1",
            "ftp://files.test/a.bin",
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
        ] {
            assert!(has_source(&source.parse().expect("url")), "{source}");
        }
        for nothing in [
            "script:fetch-links",
            "data:text/plain,x",
            "urn:isbn:0451450523",
        ] {
            assert!(!has_source(&nothing.parse().expect("url")), "{nothing}");
        }
    }
}

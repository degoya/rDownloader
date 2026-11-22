//! A hit of a subscription handed straight to a remote-job provider (RD-1460-03).
//!
//! Owner, 2026-11-22: from the indexer review box a hit goes to an account whose provider takes
//! NZB files, without a detour through the LinkGrabber. Nothing here is a second hand-over: the
//! address is the one "queue" hands the intake (`item_download_address`), the NZB behind it is
//! fetched the way the LinkGrabber's enqueue fetches an NZB link
//! (`rd_api_core::nzb_candidate::fetch_nzb`, the indexer's refusal included), and the document
//! goes through `rd_api_core::nzb_hand_over`, the path an NZB import of the LinkGrabber takes --
//! the same account checks, the same 16 MiB ceiling, the same duplicate guard. The provider is
//! given the document, never the indexer's address, so the API key in it stays on this machine.
//!
//! Afterwards the hit is decided as after "queue": its state is `queued`, and it leaves the
//! review list. A hit that could not go stays pending with nothing written, and the answer
//! carries the code that says why.

use rd_api_core::nzb_hand_over::{self, NzbHandedOver};
use rd_core::{AccountId, RemoteJob};

use super::*;

/// Which account's provider a subscription's hit goes to.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionItemRemoteJobRequest {
    /// A remote-job account whose provider takes NZB files
    /// (`GET /api/v1/remote-jobs/providers?container=nzb`).
    pub account_id: AccountId,
}

/// What handing a hit over produced.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionItemRemoteJobResponse {
    pub item_id: SubscriptionItemId,
    /// The hit's state now: `queued`, decided as after "queue".
    pub state: SubscriptionItemState,
    pub job: RemoteJob,
    /// True when the account already had a job for this NZB. Nothing was sent, and the hit is
    /// decided all the same: the content is at the provider.
    pub already_running: bool,
}

/// Hands a hit still waiting for review to one account's provider as a remote job.
///
/// Refused before anything is fetched when the hit does not exist or is decided already, and
/// when the account does not exist, runs no remote jobs or takes no NZB files. Then the hit's NZB
/// is fetched from the indexer; its refusal, or a document that is no NZB, is refused with the
/// LinkGrabber's codes (`collector.nzb_*`), and a provider's refusal with the provider's code.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/items/{id}/remote-job",
    tag = "subscriptions",
    params(("id" = SubscriptionItemId, Path)),
    request_body = SubscriptionItemRemoteJobRequest,
    responses(
        (status = 200, body = SubscriptionItemRemoteJobResponse),
        (status = 400, description = "The account's provider runs no remote jobs or takes no NZB", body = crate::error::ErrorBody),
        (status = 404, description = "The hit, its subscription or the account does not exist", body = crate::error::ErrorBody),
        (status = 409, description = "The hit is decided already", body = crate::error::ErrorBody),
        (status = 413, description = "The NZB exceeds 16 MiB", body = crate::error::ErrorBody),
        (status = 422, description = "The indexer answered with no NZB", body = crate::error::ErrorBody),
        (status = 502, description = "The indexer or the provider refused", body = crate::error::ErrorBody)
    )
)]
pub async fn submit_subscription_item_remote_job(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionItemId>,
    Json(request): Json<SubscriptionItemRemoteJobRequest>,
) -> Result<Json<SubscriptionItemRemoteJobResponse>, ApiError> {
    let (item, subscription) = items::item_with_subscription(&state, id).await?;
    if item.state != SubscriptionItemState::Pending {
        return Err(ApiError::conflict(
            "subscription.remote_job_item_decided",
            "Only a hit still waiting for review can be handed to a provider",
        ));
    }
    nzb_hand_over::ensure_takes_nzb(&state, request.account_id).await?;
    let url = items::item_download_address(&state, &subscription, &item).await?;
    let fetched = rd_api_core::nzb_candidate::fetch_nzb(
        &state,
        &url,
        rd_core::AuthProfileSelection::Auto,
        None,
    )
    .await?;
    let name = release_name(&item.title);
    // The archive password the indexer announced goes with the document when the document
    // itself names none, as "queue" hands it to the package (RD-190-04). The release's post
    // date stands in for a file without one, never the moment of the hit: the same release from
    // two hits is the same document, and the duplicate guard meets it.
    let content = match (&item.password, &fetched.document.password) {
        (Some(password), None) => {
            let mut document = fetched.document;
            document.password = Some(password.clone());
            rd_collector::render_nzb(
                &document,
                name.as_deref().map(rd_collector::container_name).as_deref(),
                item.published_at
                    .map_or(0, rd_core::Timestamp::unix_seconds),
            )
        }
        _ => fetched.fetched.bytes,
    };
    let NzbHandedOver {
        job,
        already_running,
    } = nzb_hand_over::submit_nzb(&state, request.account_id, content, name).await?;
    // After the job, never before it: a crash in between leaves the hit pending, and the next
    // press meets the duplicate guard and decides it then.
    state
        .database
        .set_subscription_item_state(id, SubscriptionItemState::Queued)
        .await
        .map_err(not_found)?;
    tracing::info!(
        subscription = %subscription.id,
        subscription_item = %id,
        remote_job = %job.id,
        account = %job.account_id,
        already_running,
        "a subscription hit was handed to a provider"
    );
    Ok(Json(SubscriptionItemRemoteJobResponse {
        item_id: id,
        state: SubscriptionItemState::Queued,
        job,
        already_running,
    }))
}

/// The name the job is listed under: the hit's title as a `.nzb` file name, without a
/// `{{password}}` marker. `None` for a title with nothing left.
fn release_name(title: &str) -> Option<String> {
    let (stripped, _) = rd_files::strip_password_marker(title);
    let stem = stripped.trim();
    if stem.is_empty() {
        return None;
    }
    rd_api_core::remote_job_service::source_name(Some(&format!(
        "{}.nzb",
        rd_files::sanitize_file_name(stem)
    )))
}

#[cfg(test)]
mod tests {
    use super::release_name;

    #[test]
    fn a_hit_is_listed_under_its_title_as_an_nzb_without_the_password_marker() {
        assert_eq!(
            release_name("Show.S01E01.1080p").as_deref(),
            Some("Show.S01E01.1080p.nzb")
        );
        assert_eq!(
            release_name("Show.S01E02{{secret}}").as_deref(),
            Some("Show.S01E02.nzb")
        );
        assert_eq!(release_name("  "), None);
    }
}

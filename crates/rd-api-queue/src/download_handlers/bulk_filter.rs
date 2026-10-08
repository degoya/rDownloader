//! The filter form of the bulk action (RD-1190-15): the files named by their state, and
//! optionally their package, instead of by a list of ids.
//!
//! "Reset every failed file" used to mean the client gathering the ids and sending them in
//! batches of 500. The filter is read once, when the action starts; a file that fails a moment
//! later is not taken, and one that left the states in between is refused like any other.

use rd_api_core::list_bounds::validate_bulk;
use rd_core::DownloadId;

use crate::{
    ApiError, AppState,
    dto::{DownloadBulkAction, DownloadBulkFilter, DownloadBulkResponse},
};

/// The bulk action on its ids (1-500), or on what its filter selects. A filter is not bounded
/// by the 500 ids, and one that selects nothing affects nothing rather than failing.
pub async fn apply_download_action_to(
    state: &AppState,
    action: DownloadBulkAction,
    ids: Vec<DownloadId>,
    filter: Option<DownloadBulkFilter>,
) -> Result<DownloadBulkResponse, ApiError> {
    if filter.is_none() {
        validate_bulk(ids.len())?;
    }
    let ids = bulk_targets(state, ids, filter).await?;
    Ok(super::act_on_ids(state, action, ids).await)
}

/// `400` for a request that names its files both ways, or by a filter without a state.
fn bulk_filter_invalid() -> ApiError {
    ApiError::bad_request(
        "request.bulk_filter",
        "Name the files either by ids or by a filter with at least one state",
    )
}

/// The ids a bulk request acts on: its own ids, or what its filter selects, in queue order.
///
/// `404` for a filter whose package does not exist, so a stale package id does not read as
/// "nothing to do".
async fn bulk_targets(
    state: &AppState,
    ids: Vec<DownloadId>,
    filter: Option<DownloadBulkFilter>,
) -> Result<Vec<DownloadId>, ApiError> {
    let Some(filter) = filter else {
        return Ok(ids);
    };
    if !ids.is_empty() || filter.states.is_empty() {
        return Err(bulk_filter_invalid());
    }
    let files = match filter.package_id {
        Some(package_id) => {
            if state.database.get_package(package_id).await?.is_none() {
                return Err(crate::error_codes::package_not_found());
            }
            state.database.downloads_for_package(package_id).await?
        }
        None => state.database.list_downloads().await?,
    };
    Ok(files
        .into_iter()
        .filter(|file| filter.states.contains(&file.state))
        .map(|file| file.id)
        .collect())
}

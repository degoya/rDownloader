//! An NZB from the LinkGrabber handed to a remote-job provider instead of the queue (RD-191-13).
//!
//! The provider fetches the articles from Usenet on its side, and what it finished comes back
//! into the LinkGrabber as direct links, the way every remote job's result does. The browser
//! does not round-trip the file: an import keeps its files, groups and articles rather than
//! the document it came from, so the server writes the NZB back out from them
//! (`rd_collector::render_nzb`) and submits it through the very path
//! `POST /api/v1/accounts/{id}/remote-jobs` takes for a container -- the same duplicate guard,
//! the same plugin `identify`, the same sweep.
//!
//! The import is not consumed. It stays in the LinkGrabber marked with the job it went to, so
//! a second press does not queue it by accident, and enqueueing it stays possible: a provider
//! that refuses the job later must not have cost the person their NZB.
//!
//! The Downloads view hands over the NZB behind a queued package the same way
//! (`POST /api/v1/packages/{id}/remote-job`), in any state of the package -- a failed one above
//! all, whose articles the own servers no longer had. The package stays; the mark is the
//! import's, as in the LinkGrabber.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_api_core::nzb_hand_over::{self, NzbHandedOver};
use rd_core::{AccountId, NzbImportId, NzbImportState, PackageId, RemoteJob};
use rd_db::StoreErrorKind;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// Which account's provider an NZB import goes to.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NzbImportRemoteJobRequest {
    /// A remote-job account whose provider takes NZB files
    /// (`GET /api/v1/remote-jobs/providers?container=nzb`).
    pub account_id: AccountId,
}

/// What handing an NZB import over produced.
#[derive(Debug, Serialize, ToSchema)]
pub struct NzbImportRemoteJobResponse {
    /// The import, now carrying `handed_over`.
    pub import: rd_core::NzbImport,
    pub job: RemoteJob,
    /// True when the account already had a job for this NZB. Nothing was written and nothing
    /// was sent -- the duplicate guard answering, not a failure.
    pub already_running: bool,
}

/// Hands an NZB import still in the LinkGrabber to one account's provider as a remote job.
///
/// Refused before anything leaves the machine when the import failed or is queued, when the
/// account does not exist, has no remote-job plugin or one that takes no NZB, and when the
/// written-out NZB exceeds the 16 MiB a remote job's container may carry.
#[utoipa::path(post, path = "/api/v1/nzb/imports/{id}/remote-job", tag = "usenet", params(("id" = NzbImportId, Path)), request_body = NzbImportRemoteJobRequest, responses((status = 200, body = NzbImportRemoteJobResponse), (status = 400, description = "The account's provider runs no remote jobs or takes no NZB"), (status = 404), (status = 409, description = "The import failed or is already queued"), (status = 413, description = "The NZB exceeds 16 MiB"), (status = 502)))]
pub async fn submit_nzb_import_remote_job(
    State(state): State<AppState>,
    Path(id): Path<NzbImportId>,
    Json(request): Json<NzbImportRemoteJobRequest>,
) -> Result<Json<NzbImportRemoteJobResponse>, ApiError> {
    let import = state
        .database
        .get_nzb_import(id)
        .await?
        .ok_or_else(import_not_found)?;
    match import.state {
        NzbImportState::Imported => {}
        NzbImportState::Enqueued => return Err(already_enqueued()),
        // The record of a drop that did not work: there are no files to hand anybody.
        NzbImportState::Failed => {
            return Err(ApiError::conflict(
                "nzb.remote_job_import_failed",
                "A failed NZB import holds no files to hand to a provider",
            ));
        }
    }
    hand_over(&state, import, request.account_id, already_enqueued)
        .await
        .map(Json)
}

/// Hands the NZB behind a package in the Downloads view to one account's provider as a remote
/// job (RD-191-13).
///
/// Whatever state the package is in -- a failed package above all, whose articles the own
/// servers no longer had. The package stays where it is, and the import behind it carries the
/// mark the Downloads view shows. Refused with `package.remote_job_no_nzb` for a package that
/// came from no NZB, or whose import history was dropped; the account and size refusals are
/// those of the LinkGrabber's route.
#[utoipa::path(post, path = "/api/v1/packages/{id}/remote-job", tag = "downloads", params(("id" = PackageId, Path)), request_body = NzbImportRemoteJobRequest, responses((status = 200, body = NzbImportRemoteJobResponse), (status = 400, description = "The package has no NZB behind it, or the account's provider runs no remote jobs or takes no NZB"), (status = 404), (status = 413, description = "The NZB exceeds 16 MiB"), (status = 502)))]
pub async fn submit_package_remote_job(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
    Json(request): Json<NzbImportRemoteJobRequest>,
) -> Result<Json<NzbImportRemoteJobResponse>, ApiError> {
    let package = state
        .database
        .get_package(id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    let import = match package.nzb_import_id {
        Some(import_id) => state.database.get_nzb_import(import_id).await?,
        None => None,
    }
    .filter(|import| import.state == NzbImportState::Enqueued)
    .ok_or_else(package_without_nzb)?;
    hand_over(&state, import, request.account_id, package_without_nzb)
        .await
        .map(Json)
}

/// The checks, the written-out NZB, the job and the mark both routes share. `moved_on` answers
/// an import that left the state it was handed over from while the job was being submitted.
async fn hand_over(
    state: &AppState,
    import: rd_core::NzbImport,
    account_id: AccountId,
    moved_on: fn() -> ApiError,
) -> Result<NzbImportRemoteJobResponse, ApiError> {
    let id = import.id;
    // The account checks and the submission are `rd_api_core::nzb_hand_over`'s, shared with
    // the hand-over of an indexer subscription's hit (RD-1460-03).
    nzb_hand_over::ensure_takes_nzb(state, account_id).await?;
    let files = state.database.list_nzb_files(id).await?;
    let content = rd_collector::render_nzb(
        &document(import.password.clone(), files),
        Some(&rd_collector::container_name(&import.name)),
        import.created_at.unix_seconds(),
    );
    let NzbHandedOver {
        job,
        already_running,
    } = nzb_hand_over::submit_nzb(state, account_id, content, Some(import.name.clone())).await?;
    // After the row, never before it: a crash in between leaves a job without its mark, and
    // the next press meets the duplicate guard above and writes the mark then.
    let import = state
        .database
        .mark_nzb_import_remote_job(id, job.id, import.state)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => import_not_found(),
            Some(StoreErrorKind::WrongState) => moved_on(),
            _ => error.into(),
        })?;
    tracing::info!(
        nzb_import = %id,
        remote_job = %job.id,
        account = %job.account_id,
        already_running,
        "an NZB import was handed to a provider"
    );
    Ok(NzbImportRemoteJobResponse {
        import,
        job,
        already_running,
    })
}

/// The stored files of an import as the document they were parsed from.
pub(crate) fn document(
    password: Option<String>,
    files: Vec<rd_core::NzbFileStatus>,
) -> rd_collector::NzbDocument {
    rd_collector::NzbDocument {
        password,
        files: files
            .into_iter()
            .map(|file| rd_collector::NzbFile {
                subject: file.subject,
                poster: file.poster,
                date: None,
                groups: file.groups,
                segments: file
                    .segments
                    .into_iter()
                    .map(|segment| rd_collector::NzbSegment {
                        number: segment.number,
                        bytes: segment.bytes.get(),
                        message_id: segment.message_id,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn import_not_found() -> ApiError {
    ApiError::not_found("nzb.import_not_found", "NZB import not found")
}

fn package_without_nzb() -> ApiError {
    ApiError::bad_request(
        "package.remote_job_no_nzb",
        "This package has no NZB behind it to hand to a provider",
    )
}

fn already_enqueued() -> ApiError {
    ApiError::conflict(
        "nzb.already_enqueued",
        "NZB import is already in the download list",
    )
}

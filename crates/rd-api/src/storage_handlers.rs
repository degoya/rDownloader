//! Dedupe links, the storage history, the runners' reuse declarations and the content index
//! (RD-150-02).
//!
//! Every action here is explicit: nothing replaces a file by a link unless it is asked to, the
//! request names both files, and the link is made only after both were hashed. Each attempt
//! lands in the storage history and, as a trust-relevant change to somebody's files, in the
//! audit log.

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};
use chrono::{DateTime, Utc};
use rd_core::{
    DownloadId, DownloadKind, DownloadState, PackageId, ReuseCapability, StorageOperationKind,
    StorageOperationState,
};
use rd_files::{LinkError, StorageTarget};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, error::ApiError, error_codes::parse_id};

#[derive(Serialize, ToSchema)]
pub struct RunnerReuseResponse {
    pub kind: DownloadKind,
    pub capability: ReuseCapability,
}

/// What every transfer kind this service runs can reuse of data on disk.
#[utoipa::path(get, path = "/api/v1/storage/reuse", tag = "system", responses((status = 200, body = [RunnerReuseResponse])))]
pub async fn reuse_capabilities(State(state): State<AppState>) -> Json<Vec<RunnerReuseResponse>> {
    Json(
        state
            .scheduler
            .reuse_capabilities()
            .into_iter()
            .map(|(kind, capability)| RunnerReuseResponse { kind, capability })
            .collect(),
    )
}

/// One storage root's link capabilities, probed by trying.
#[derive(Serialize, ToSchema)]
pub struct LinkSupportEntry {
    /// `None` for the service download directory.
    pub storage_root_id: Option<rd_core::StorageRootId>,
    pub name: String,
    pub path: String,
    pub hardlink: bool,
    /// Always `false` in this build: no reflink is created without `unsafe` code.
    pub reflink: bool,
}

#[utoipa::path(get, path = "/api/v1/storage/link-support", tag = "system", responses((status = 200, body = [LinkSupportEntry])))]
pub async fn link_support(
    State(state): State<AppState>,
) -> Result<Json<Vec<LinkSupportEntry>>, ApiError> {
    let names = state
        .database
        .list_storage_roots()
        .await?
        .into_iter()
        .map(|root| (root.id, root.name))
        .collect::<std::collections::HashMap<_, _>>();
    let mut entries = Vec::new();
    for (target, path) in state.scheduler.capacity().targets().await {
        let support = rd_files::probe_link_support(&path).await;
        entries.push(LinkSupportEntry {
            storage_root_id: match target {
                StorageTarget::Root(id) => Some(id),
                StorageTarget::Fallback => None,
            },
            name: match target {
                StorageTarget::Root(id) => names.get(&id).cloned().unwrap_or_default(),
                StorageTarget::Fallback => "Downloads".to_owned(),
            },
            path: path.to_string_lossy().into_owned(),
            hardlink: support.hardlink,
            reflink: support.reflink,
        });
    }
    Ok(Json(entries))
}

#[derive(Deserialize, IntoParams)]
pub struct StorageOperationsQuery {
    /// Newest rows to return (1-1000, default 100).
    pub limit: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct StorageOperationResponse {
    pub id: i64,
    pub kind: StorageOperationKind,
    pub state: StorageOperationState,
    pub package_id: Option<PackageId>,
    pub download_id: Option<DownloadId>,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: Option<u64>,
    /// SHA-256 both copies were verified to share.
    pub verified_digest: Option<String>,
    /// Stable code of a failure.
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl From<rd_db::StorageOperation> for StorageOperationResponse {
    fn from(operation: rd_db::StorageOperation) -> Self {
        Self {
            id: operation.id,
            kind: operation.kind,
            state: operation.state,
            package_id: operation.package_id,
            download_id: operation.download_id,
            source_path: operation.source_path,
            target_path: operation.target_path,
            size_bytes: operation.size_bytes,
            verified_digest: operation.verified_digest,
            error_code: operation.error_code,
            error_message: operation.error_message,
            started_at: operation.started_at,
            finished_at: operation.finished_at,
        }
    }
}

/// The history of verified moves and dedupe links, newest first.
#[utoipa::path(get, path = "/api/v1/storage/operations", tag = "system", params(StorageOperationsQuery), responses((status = 200, body = [StorageOperationResponse]), (status = 400)))]
pub async fn list_storage_operations(
    State(state): State<AppState>,
    Query(query): Query<StorageOperationsQuery>,
) -> Result<Json<Vec<StorageOperationResponse>>, ApiError> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=1000).contains(&limit) {
        return Err(ApiError::bad_request(
            "storage.operations_limit_invalid",
            "The limit must be between 1 and 1000",
        ));
    }
    Ok(Json(
        state
            .database
            .list_storage_operations(limit)
            .await?
            .into_iter()
            .map(StorageOperationResponse::from)
            .collect(),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct ContentIndexCheckResponse {
    pub checked: u64,
    pub missing: u64,
    pub restored: u64,
    pub backfilled: u64,
}

/// Checks the content index against the disk and the queue now, rather than at the next start.
#[utoipa::path(post, path = "/api/v1/storage/content-index/check", tag = "system", responses((status = 200, body = ContentIndexCheckResponse)))]
pub async fn check_content_index(
    State(state): State<AppState>,
) -> Result<Json<ContentIndexCheckResponse>, ApiError> {
    let report = state.scheduler.check_content_index().await?;
    let count = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
    Ok(Json(ContentIndexCheckResponse {
        checked: count(report.checked),
        missing: count(report.missing),
        restored: count(report.restored),
        backfilled: count(report.backfilled),
    }))
}

/// How a duplicate is replaced.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DedupeMode {
    Hardlink,
    /// Refused in this build; accepted in the contract so the refusal is a stated code.
    Reflink,
}

#[derive(Deserialize, ToSchema)]
pub struct DedupeRequest {
    /// The download whose file stays; the one in the path becomes a link to it.
    pub original_download_id: DownloadId,
    pub mode: DedupeMode,
}

#[derive(Serialize, ToSchema)]
pub struct DedupeResponse {
    pub operation_id: i64,
    /// SHA-256 both files were verified to share.
    pub digest: String,
    /// Bytes the duplicate no longer occupies on its own.
    pub freed_bytes: u64,
}

/// Replaces a download's finished file by a hard link to an identical original.
#[utoipa::path(post, path = "/api/v1/downloads/{id}/dedupe", tag = "downloads", params(("id" = String, Path)), request_body = DedupeRequest, responses((status = 200, body = DedupeResponse), (status = 404), (status = 409), (status = 422)))]
pub async fn dedupe_download(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<DedupeRequest>,
) -> Result<Json<DedupeResponse>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    if request.mode == DedupeMode::Reflink {
        return Err(ApiError::unprocessable(
            "storage.reflink_unsupported",
            "This build creates hard links only",
        ));
    }
    if request.original_download_id == id {
        return Err(ApiError::bad_request(
            "storage.dedupe_same_download",
            "A download cannot be linked to itself",
        ));
    }
    let duplicate = finished_entry(&state, id).await?;
    let original = finished_entry(&state, request.original_download_id).await?;
    if duplicate.algorithm != original.algorithm || duplicate.digest != original.digest {
        return Err(ApiError::conflict(
            "storage.dedupe_not_duplicates",
            "The index does not list these two files as identical",
        ));
    }
    for entry in [&duplicate, &original] {
        if state
            .scheduler
            .file_in_use(std::path::Path::new(&entry.path), None)
            .await?
        {
            return Err(ApiError::conflict(
                "storage.dedupe_in_use",
                "One of the files belongs to a transfer that is running or seeding",
            ));
        }
    }

    let operation = state
        .database
        .start_storage_operation(rd_db::NewStorageOperation {
            kind: StorageOperationKind::Dedupe,
            package_id: None,
            download_id: Some(id),
            source_path: original.path.clone(),
            target_path: duplicate.path.clone(),
            size_bytes: Some(duplicate.size_bytes),
        })
        .await?;
    let result = rd_files::link_duplicate(
        std::path::Path::new(&original.path),
        std::path::Path::new(&duplicate.path),
    )
    .await;
    let outcome = match &result {
        Ok(linked) => rd_db::StorageOperationOutcome::completed(
            Some(linked.size_bytes),
            Some(linked.digest.clone()),
        ),
        Err(error) => rd_db::StorageOperationOutcome::failed(error.code(), error.to_string()),
    };
    state
        .database
        .finish_storage_operation(operation, outcome)
        .await?;
    let event = match &result {
        Ok(_) => crate::audit::AuditEvent::success(rd_core::AuditAction::DuplicateLinked),
        Err(error) => crate::audit::AuditEvent::failure(rd_core::AuditAction::DuplicateLinked)
            .detail("code", error.code()),
    };
    crate::audit::record(
        &state,
        event
            .by(&audit)
            .target("download", id)
            .detail("original_download_id", request.original_download_id)
            .detail("path", &duplicate.path),
    )
    .await;
    match result {
        Ok(linked) => Ok(Json(DedupeResponse {
            operation_id: operation,
            digest: linked.digest,
            freed_bytes: linked.size_bytes,
        })),
        Err(error) => Err(link_refusal(&error)),
    }
}

/// The index entry of a finished download's file.
async fn finished_entry(
    state: &AppState,
    id: DownloadId,
) -> Result<rd_db::ContentIndexEntry, ApiError> {
    let download = state
        .database
        .get_download(id)
        .await?
        .ok_or_else(crate::error_codes::download_not_found)?;
    if download.state != DownloadState::Completed {
        return Err(ApiError::conflict(
            "storage.dedupe_not_finished",
            "Only finished downloads can be linked",
        ));
    }
    state
        .database
        .content_index_entry(id)
        .await?
        .filter(|entry| entry.missing_since.is_none())
        .ok_or_else(|| {
            ApiError::conflict(
                "storage.dedupe_not_indexed",
                "The download's file has no verified hash in the content index",
            )
        })
}

fn link_refusal(error: &LinkError) -> ApiError {
    match error {
        LinkError::ContentDiffers(..) => ApiError::conflict(
            "storage.dedupe_content_differs",
            "The files do not hold the same bytes",
        ),
        LinkError::AlreadyLinked(..) => ApiError::conflict(
            "storage.dedupe_already_linked",
            "The files are already linked",
        ),
        LinkError::Unsupported(..) => ApiError::unprocessable(
            "storage.dedupe_link_unsupported",
            "The file system cannot link these two files",
        ),
        LinkError::Failed(..) => ApiError::from(anyhow::anyhow!(error.to_string())),
    }
}

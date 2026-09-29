//! The full backup's destinations, the archives written to them and their verification
//! (RD-160-02).
//!
//! A destination is a local folder or NAS path, a folder of an object storage bucket reached
//! through one of the profiles of `/api/v1/object-storage/profiles`, or an rclone remote —
//! WebDAV included, which has no destination of its own (owner's decision, 2026-09-28). None
//! of them carries a secret: a profile is named by its id, an rclone remote by its name, and
//! their credentials stay where they are configured. Each destination keeps its own retention,
//! which can be previewed without deleting anything. Every route costs `api:admin`
//! (`scope_policy`), like the rest of the full backup.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use rd_backup::{DestinationConfig, RecordedArchive, RetentionPolicy, retention};
use rd_core::{AuditAction, BackupOrigin, BackupVerifyState};
use rd_db::{BackupArchive, BackupDestinationRecord, BackupVerification, NewBackupDestination};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::{AuditContext, AuditEvent};
use crate::dto::MessageResponse;
use crate::{ApiError, AppState, backup_delivery};

/// How many verifications the history route answers with.
const VERIFICATIONS_LISTED: u32 = 50;
/// The longest name a destination may have.
const MAX_NAME_CHARS: usize = 120;

/// A destination as the interface shows it.
#[derive(Serialize, ToSchema)]
pub struct BackupDestinationResponse {
    pub id: String,
    /// `local`, `object_storage` or `rclone`.
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    /// The folder of a `local` destination.
    pub path: Option<String>,
    /// The object storage profile of an `object_storage` destination.
    pub profile_id: Option<String>,
    /// `<bucket>/<folder>` of an `object_storage` destination; an empty bucket is the
    /// profile's bound one.
    pub prefix: Option<String>,
    /// `name:path` of an `rclone` destination.
    pub remote: Option<String>,
    /// The newest archives kept; `None` is no limit by count.
    pub keep_last: Option<u32>,
    /// The days archives are kept; `None` is no limit by age.
    pub keep_days: Option<u32>,
    /// Archives this installation recorded there.
    pub archive_count: u32,
    pub last_stored_at: Option<DateTime<Utc>>,
    /// The last verification of the newest archive there.
    pub last_verify_state: Option<BackupVerifyState>,
}

/// A destination to create or replace. Only the fields of its kind are read.
#[derive(Deserialize, ToSchema)]
pub struct BackupDestinationRequest {
    /// `local`, `object_storage` or `rclone`.
    pub kind: String,
    /// How the interface and the history name it; empty is the destination's own address.
    #[serde(default)]
    pub name: Option<String>,
    /// Missing is on.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// `local`: an absolute folder, checked like a storage root and created when missing.
    #[serde(default)]
    pub path: Option<String>,
    /// `object_storage`: the profile's id.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// `object_storage`: `<bucket>/<folder>`.
    #[serde(default)]
    pub prefix: Option<String>,
    /// `rclone`: `name:path` of a configured remote.
    #[serde(default)]
    pub remote: Option<String>,
    /// At least 1; missing is no limit by count.
    #[serde(default)]
    pub keep_last: Option<u32>,
    /// At least 1; missing is no limit by age.
    #[serde(default)]
    pub keep_days: Option<u32>,
}

/// One archive of the ledger.
#[derive(Clone, Serialize, ToSchema)]
pub struct BackupArchiveResponse {
    pub id: String,
    pub destination_id: String,
    pub run_id: String,
    pub archive_name: String,
    pub location: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
    pub stored_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
    pub verify_state: Option<BackupVerifyState>,
    pub verify_code: Option<String>,
}

impl From<&BackupArchive> for BackupArchiveResponse {
    fn from(archive: &BackupArchive) -> Self {
        Self {
            id: archive.id.clone(),
            destination_id: archive.destination_id.clone(),
            run_id: archive.run_id.clone(),
            archive_name: archive.archive_name.clone(),
            location: archive.location.clone(),
            size_bytes: archive.size_bytes,
            sha256: archive.sha256.clone(),
            created_at: archive.created_at,
            stored_at: archive.stored_at,
            verified_at: archive.verified_at,
            verify_state: archive.verify_state,
            verify_code: archive.verify_code.clone(),
        }
    }
}

/// What a retention pass would do at a destination; nothing is deleted by asking.
#[derive(Serialize, ToSchema)]
pub struct RetentionPreviewResponse {
    pub keep_last: Option<u32>,
    pub keep_days: Option<u32>,
    /// Newest first.
    pub keep: Vec<BackupArchiveResponse>,
    pub remove: Vec<BackupArchiveResponse>,
}

/// A policy to preview instead of the stored one, for a form that is being edited.
#[derive(Debug, Deserialize)]
pub struct RetentionPreviewQuery {
    #[serde(default)]
    pub keep_last: Option<u32>,
    #[serde(default)]
    pub keep_days: Option<u32>,
}

/// Filters the ledger to one destination.
#[derive(Debug, Deserialize)]
pub struct ArchiveQuery {
    #[serde(default)]
    pub destination_id: Option<String>,
}

/// One verification of one archive.
#[derive(Serialize, ToSchema)]
pub struct BackupVerificationResponse {
    pub id: String,
    pub origin: BackupOrigin,
    pub state: BackupVerifyState,
    pub archive_id: Option<String>,
    pub destination_id: Option<String>,
    pub destination: String,
    pub archive_name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Whether the archive was opened and every member checked, or only its size and SHA-256
    /// compared (an archive sealed under an earlier passphrase).
    pub content_checked: Option<bool>,
    /// `backup.verify_missing`, `backup.verify_digest_mismatch`, `backup.verify_damaged` or
    /// the destination's own code.
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

impl From<BackupVerification> for BackupVerificationResponse {
    fn from(row: BackupVerification) -> Self {
        Self {
            id: row.id,
            origin: row.origin,
            state: row.state,
            archive_id: row.archive_id,
            destination_id: row.destination_id,
            destination: row.destination,
            archive_name: row.archive_name,
            started_at: row.started_at,
            finished_at: row.finished_at,
            content_checked: row.content_checked,
            error_code: row.error_code,
            error_detail: row.error_detail,
        }
    }
}

fn text(config: &serde_json::Value, field: &str) -> Option<String> {
    config
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// A destination row as the interface shows it, with what the ledger knows of it.
#[must_use]
pub fn destination_response(
    record: &BackupDestinationRecord,
    archives: &[BackupArchive],
) -> BackupDestinationResponse {
    let own: Vec<&BackupArchive> = archives
        .iter()
        .filter(|archive| archive.destination_id == record.id)
        .collect();
    let newest = own.iter().max_by_key(|archive| archive.created_at);
    BackupDestinationResponse {
        id: record.id.clone(),
        kind: record.kind.clone(),
        name: backup_delivery::label(record),
        enabled: record.enabled,
        path: text(&record.config, "path"),
        profile_id: text(&record.config, "profile_id"),
        prefix: text(&record.config, "prefix"),
        remote: text(&record.config, "remote"),
        keep_last: record.keep_last,
        keep_days: record.keep_days,
        archive_count: u32::try_from(own.len()).unwrap_or(u32::MAX),
        last_stored_at: newest.map(|archive| archive.stored_at),
        last_verify_state: newest.and_then(|archive| archive.verify_state),
    }
}

fn not_found() -> ApiError {
    ApiError::not_found(
        "backup.destination_not_found",
        "No backup destination has this id",
    )
}

fn retention_policy(
    keep_last: Option<u32>,
    keep_days: Option<u32>,
) -> Result<RetentionPolicy, ApiError> {
    if keep_last == Some(0) || keep_days == Some(0) {
        return Err(ApiError::bad_request(
            "backup.retention_invalid",
            "Keep at least one archive and at least one day",
        ));
    }
    Ok(RetentionPolicy {
        keep_last,
        keep_days,
    })
}

/// Reads and checks a request into the row it saves.
async fn new_destination(
    state: &AppState,
    request: BackupDestinationRequest,
) -> Result<NewBackupDestination, ApiError> {
    let field = |value: Option<String>| {
        value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let config = match request.kind.as_str() {
        rd_backup::LocalFolder::KIND => DestinationConfig::Local {
            path: field(request.path)
                .ok_or_else(|| {
                    ApiError::bad_request("backup.destination_path_missing", "Name a folder")
                })?
                .into(),
        },
        rd_backup::remote::OBJECT_STORAGE_KIND => DestinationConfig::ObjectStorage {
            profile_id: field(request.profile_id).ok_or_else(|| {
                ApiError::bad_request(
                    "backup.destination_profile_missing",
                    "Choose an object storage profile",
                )
            })?,
            prefix: field(request.prefix).unwrap_or_default(),
        },
        rd_backup::remote::RCLONE_KIND => DestinationConfig::Rclone {
            remote: field(request.remote).ok_or_else(|| {
                ApiError::bad_request(
                    rd_backup::remote::RCLONE_REMOTE_INVALID,
                    "Name an rclone remote as name:path",
                )
            })?,
        },
        other => {
            return Err(ApiError::unprocessable(
                rd_backup::remote::DESTINATION_KIND_UNKNOWN,
                "The destination kind is not local, object_storage or rclone",
            )
            .with_param("value", other.chars().take(32).collect::<String>()));
        }
    };
    let policy = retention_policy(request.keep_last, request.keep_days)?;
    // Everything that can be checked without the network: the folder like a storage root, the
    // profile and its bucket, the shape of the remote.
    let context = backup_delivery::destination_context(state).await;
    config
        .validate(&context)
        .await
        .map_err(|error| ApiError::bad_request(error.code(), error.to_string()))?;
    let name = field(request.name).unwrap_or_else(|| config.describe());
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::bad_request(
            "backup.destination_name_too_long",
            format!("A destination name has at most {MAX_NAME_CHARS} characters"),
        )
        .with_param("max", MAX_NAME_CHARS));
    }
    Ok(NewBackupDestination {
        kind: config.kind().to_owned(),
        name,
        config: config.to_json(),
        enabled: request.enabled.unwrap_or(true),
        keep_last: policy.keep_last,
        keep_days: policy.keep_days,
    })
}

/// Refuses to leave a switched-on schedule without an enabled destination.
async fn keep_one_enabled(state: &AppState, without: &str) -> Result<(), ApiError> {
    let config = state.database.backup_config().await?;
    if config.enabled
        && !config
            .destinations
            .iter()
            .any(|record| record.enabled && record.id != without)
    {
        return Err(ApiError::conflict(
            "backup.destination_last",
            "The schedule is on; switch it off before removing its last destination",
        ));
    }
    Ok(())
}

async fn response_of(state: &AppState, id: &str) -> Result<BackupDestinationResponse, ApiError> {
    let record = state
        .database
        .backup_destination(id)
        .await?
        .ok_or_else(not_found)?;
    let archives = state.database.backup_archives(Some(id)).await?;
    Ok(destination_response(&record, &archives))
}

async fn audit_change(state: &AppState, audit: &AuditContext, id: &str, kind: &str, change: &str) {
    crate::audit::record(
        state,
        AuditEvent::success(AuditAction::BackupConfigured)
            .by(audit)
            .target("backup_destination", id)
            .detail("kind", kind)
            .detail("change", change),
    )
    .await;
}

#[utoipa::path(
    post,
    path = "/api/v1/backups/destinations",
    tag = "system",
    request_body = BackupDestinationRequest,
    responses((status = 201, body = BackupDestinationResponse), (status = 400, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody))
)]
pub async fn create_backup_destination(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<BackupDestinationRequest>,
) -> Result<(StatusCode, Json<BackupDestinationResponse>), ApiError> {
    let destination = new_destination(&state, request).await?;
    let kind = destination.kind.clone();
    let id = state
        .database
        .create_backup_destination(destination)
        .await?;
    audit_change(&state, &audit, &id, &kind, "created").await;
    Ok((StatusCode::CREATED, Json(response_of(&state, &id).await?)))
}

#[utoipa::path(
    put,
    path = "/api/v1/backups/destinations/{id}",
    tag = "system",
    params(("id" = String, Path, description = "The destination's id")),
    request_body = BackupDestinationRequest,
    responses((status = 200, body = BackupDestinationResponse), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody))
)]
pub async fn update_backup_destination(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<BackupDestinationRequest>,
) -> Result<Json<BackupDestinationResponse>, ApiError> {
    if state.database.backup_destination(&id).await?.is_none() {
        return Err(not_found());
    }
    let destination = new_destination(&state, request).await?;
    if !destination.enabled {
        keep_one_enabled(&state, &id).await?;
    }
    let kind = destination.kind.clone();
    if !state
        .database
        .update_backup_destination(id.clone(), destination)
        .await?
    {
        return Err(not_found());
    }
    audit_change(&state, &audit, &id, &kind, "updated").await;
    Ok(Json(response_of(&state, &id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/v1/backups/destinations/{id}",
    tag = "system",
    params(("id" = String, Path, description = "The destination's id")),
    responses((status = 200, body = MessageResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody))
)]
pub async fn delete_backup_destination(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let record = state
        .database
        .backup_destination(&id)
        .await?
        .ok_or_else(not_found)?;
    keep_one_enabled(&state, &id).await?;
    // The archives stay where they are; only this installation's record of them goes.
    if !state.database.delete_backup_destination(id.clone()).await? {
        return Err(not_found());
    }
    audit_change(&state, &audit, &id, &record.kind, "deleted").await;
    Ok(Json(MessageResponse::new(
        "backup.destination_deleted",
        "Backup destination removed; its archives stay where they are",
    )))
}

#[utoipa::path(
    get,
    path = "/api/v1/backups/destinations/{id}/retention",
    tag = "system",
    params(
        ("id" = String, Path, description = "The destination's id"),
        ("keep_last" = Option<u32>, Query, description = "Preview this count instead of the stored one"),
        ("keep_days" = Option<u32>, Query, description = "Preview this age in days instead of the stored one")
    ),
    responses((status = 200, body = RetentionPreviewResponse), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody))
)]
pub async fn preview_backup_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RetentionPreviewQuery>,
) -> Result<Json<RetentionPreviewResponse>, ApiError> {
    let record = state
        .database
        .backup_destination(&id)
        .await?
        .ok_or_else(not_found)?;
    let asked = query.keep_last.is_some() || query.keep_days.is_some();
    let policy = if asked {
        retention_policy(query.keep_last, query.keep_days)?
    } else {
        RetentionPolicy {
            keep_last: record.keep_last,
            keep_days: record.keep_days,
        }
    };
    let instance = state.database.backup_config().await?.instance_id;
    let ledger = state.database.backup_archives(Some(&id)).await?;
    let recorded: Vec<RecordedArchive> = ledger
        .iter()
        .map(|archive| RecordedArchive {
            id: archive.id.clone(),
            name: archive.archive_name.clone(),
            created_at: archive.created_at,
        })
        .collect();
    let plan = retention::plan(&recorded, policy, &instance, Utc::now());
    let pick = |ids: &[String]| -> Vec<BackupArchiveResponse> {
        ids.iter()
            .filter_map(|id| ledger.iter().find(|archive| &archive.id == id))
            .map(BackupArchiveResponse::from)
            .collect()
    };
    Ok(Json(RetentionPreviewResponse {
        keep_last: policy.keep_last,
        keep_days: policy.keep_days,
        keep: pick(&plan.keep),
        remove: pick(&plan.remove),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/backups/archives",
    tag = "system",
    params(("destination_id" = Option<String>, Query, description = "Only this destination's archives")),
    responses((status = 200, body = [BackupArchiveResponse]))
)]
pub async fn list_backup_archives(
    State(state): State<AppState>,
    Query(query): Query<ArchiveQuery>,
) -> Result<Json<Vec<BackupArchiveResponse>>, ApiError> {
    let archives = state
        .database
        .backup_archives(query.destination_id.as_deref())
        .await?;
    Ok(Json(archives.iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/api/v1/backups/archives/{id}/verify",
    tag = "system",
    params(("id" = String, Path, description = "The archive's id in the ledger")),
    responses((status = 202, body = BackupVerificationResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody))
)]
pub async fn verify_backup_archive(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<BackupVerificationResponse>), ApiError> {
    let archive = state.database.backup_archive(&id).await?.ok_or_else(|| {
        ApiError::not_found(
            "backup.archive_not_found",
            "No recorded archive has this id",
        )
    })?;
    let verification = crate::backup_verify_service::start_verification(
        &state,
        &archive,
        BackupOrigin::Manual,
        audit,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(verification.into())))
}

#[utoipa::path(get, path = "/api/v1/backups/verifications", tag = "system", responses((status = 200, body = [BackupVerificationResponse])))]
pub async fn list_backup_verifications(
    State(state): State<AppState>,
) -> Result<Json<Vec<BackupVerificationResponse>>, ApiError> {
    Ok(Json(
        state
            .database
            .backup_verifications(VERIFICATIONS_LISTED)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

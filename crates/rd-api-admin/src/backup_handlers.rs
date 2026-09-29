//! The full backup's REST surface (RD-160-01): the schedule, the passphrase, a run by hand,
//! and the history. The destinations, the archives and their verification are in
//! `backup_destination_handlers` (RD-160-02).
//!
//! The passphrase goes in once and never comes out: it is turned into the key right here, the
//! key goes to the secret store, and every answer says only whether a key is set and its
//! fingerprint. Replacing it asks for the current one. Every route costs `api:admin` (`scope_policy`).

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use rd_backup::{BackupKey, MIN_PASSPHRASE_CHARS, schedule};
use rd_core::{AuditAction, BackupOrigin, BackupRunState};
use rd_db::{BackupConfigUpdate, BackupKeyRecord};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::{AuditContext, AuditEvent};
use crate::backup_destination_handlers::{BackupDestinationResponse, destination_response};
use crate::{ApiError, AppState};

/// How many runs the history route answers with.
const RUNS_LISTED: u32 = 50;

/// The backup configuration as the interface shows it. The key is never part of it.
#[derive(Serialize, ToSchema)]
pub struct BackupConfigResponse {
    pub enabled: bool,
    /// Five-field cron expression, read in `timezone`.
    pub schedule: String,
    /// IANA time zone the schedule is read in.
    pub timezone: String,
    /// Every destination; each receives its own copy of every archive (RD-160-02).
    pub destinations: Vec<BackupDestinationResponse>,
    /// Five-field cron expression of the scheduled verification, read in `timezone`; `None`
    /// is off.
    pub verify_schedule: Option<String>,
    /// When the verification schedule next checks the newest archive of every destination.
    pub verify_next_run_at: Option<DateTime<Utc>>,
    /// This installation's id, part of every archive name; retention removes only archives
    /// carrying it.
    pub instance_id: String,
    /// Whether a passphrase has been set up.
    pub key_configured: bool,
    /// Sixteen hex characters telling keys apart; not the key.
    pub key_fingerprint: Option<String>,
    pub key_set_at: Option<DateTime<Utc>>,
    /// When the schedule next runs; `None` while it is off.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Whether a run is going on right now.
    pub running: bool,
}

/// The schedule and the verification schedule, saved together. Destinations have their own
/// routes.
#[derive(Deserialize, ToSchema)]
pub struct UpdateBackupConfigRequest {
    pub enabled: bool,
    pub schedule: String,
    pub timezone: String,
    /// Five-field cron expression of the scheduled verification; empty or missing is off.
    #[serde(default)]
    pub verify_schedule: Option<String>,
}

/// Sets up or replaces the passphrase. Archives written before keep the old one.
#[derive(Deserialize, ToSchema)]
pub struct SetBackupPassphraseRequest {
    #[schema(write_only)]
    pub passphrase: String,
    /// The passphrase in force; required once one is set up (owner's decision, 2026-09-28).
    #[serde(default)]
    #[schema(write_only)]
    pub current_passphrase: Option<String>,
}

/// One member of a finished archive.
#[derive(Serialize, ToSchema)]
pub struct BackupPartResponse {
    pub name: String,
    /// `settings`, `database`, `plugin_trust`, `partial_transfers`, `torrent_session` or
    /// `torrent_file`.
    pub kind: String,
    pub size: u64,
    pub sha256: String,
}

/// One run of the history.
#[derive(Serialize, ToSchema)]
pub struct BackupRunResponse {
    pub id: String,
    pub origin: BackupOrigin,
    pub state: BackupRunState,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// The destination as it was named when the run started.
    pub destination: Option<String>,
    pub archive_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub parts: Vec<BackupPartResponse>,
    /// Stable code of a failed or interrupted run, or `backup.destinations_partial` when a
    /// run reached some destinations and not others.
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    /// How each destination fared (RD-160-02).
    pub destinations: Vec<BackupRunDestinationResponse>,
}

/// One destination of a run.
#[derive(Serialize, ToSchema)]
pub struct BackupRunDestinationResponse {
    pub destination_id: String,
    /// `local`, `object_storage` or `rclone`.
    pub kind: String,
    /// The destination as it was named when the run started.
    pub destination: String,
    pub state: BackupRunState,
    /// Attempts made; an outage is tried again with a growing pause.
    pub attempts: u32,
    pub location: Option<String>,
    /// Older archives retention removed there after this run.
    pub pruned: u32,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl From<rd_db::BackupRun> for BackupRunResponse {
    fn from(run: rd_db::BackupRun) -> Self {
        let parts = run
            .parts
            .and_then(|parts| serde_json::from_value::<Vec<rd_backup::ManifestPart>>(parts).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|part| BackupPartResponse {
                kind: serde_json::to_value(part.kind)
                    .ok()
                    .and_then(|kind| kind.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                name: part.name,
                size: part.size,
                sha256: part.sha256,
            })
            .collect();
        Self {
            id: run.id,
            origin: run.origin,
            state: run.state,
            started_at: run.started_at,
            finished_at: run.finished_at,
            destination: run.destination,
            archive_name: run.archive_name,
            size_bytes: run.size_bytes,
            sha256: run.sha256,
            parts,
            error_code: run.error_code,
            error_detail: run.error_detail,
            destinations: run
                .destinations
                .into_iter()
                .map(|destination| BackupRunDestinationResponse {
                    destination_id: destination.destination_id,
                    kind: destination.kind,
                    destination: destination.destination,
                    state: destination.state,
                    attempts: destination.attempts,
                    location: destination.location,
                    pruned: destination.pruned,
                    error_code: destination.error_code,
                    error_detail: destination.error_detail,
                    finished_at: destination.finished_at,
                })
                .collect(),
        }
    }
}

pub(crate) async fn config_response(state: &AppState) -> Result<BackupConfigResponse, ApiError> {
    let config = state.database.backup_config().await?;
    let running = state
        .database
        .backup_runs(1)
        .await?
        .first()
        .is_some_and(|run| run.state == BackupRunState::Running);
    let archives = state.database.backup_archives(None).await?;
    Ok(BackupConfigResponse {
        enabled: config.enabled,
        schedule: config.schedule,
        timezone: config.timezone,
        destinations: config
            .destinations
            .iter()
            .map(|record| destination_response(record, &archives))
            .collect(),
        verify_schedule: config.verify_schedule,
        verify_next_run_at: config.verify_next_run_at,
        instance_id: config.instance_id,
        key_configured: config.key.is_some(),
        key_fingerprint: config.key.as_ref().map(|key| key.fingerprint.clone()),
        key_set_at: config.key.as_ref().map(|key| key.set_at),
        next_run_at: config.next_run_at,
        running,
    })
}

#[utoipa::path(get, path = "/api/v1/backups", tag = "system", responses((status = 200, body = BackupConfigResponse)))]
pub async fn get_backup_config(
    State(state): State<AppState>,
) -> Result<Json<BackupConfigResponse>, ApiError> {
    Ok(Json(config_response(&state).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/backups",
    tag = "system",
    request_body = UpdateBackupConfigRequest,
    responses((status = 200, body = BackupConfigResponse), (status = 400, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody))
)]
pub async fn update_backup_config(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<UpdateBackupConfigRequest>,
) -> Result<Json<BackupConfigResponse>, ApiError> {
    let current = state.database.backup_config().await?;
    let expression = request.schedule.trim().to_owned();
    let timezone = request.timezone.trim().to_owned();
    if schedule::parse_timezone(&timezone).is_none() {
        return Err(ApiError::unprocessable(
            "backup.timezone_invalid",
            "The time zone is not an IANA time zone name",
        )
        .with_param("value", timezone.chars().take(64).collect::<String>()));
    }
    let now = Utc::now();
    let next = schedule::next_run(&expression, &timezone, now).map_err(|error| {
        ApiError::unprocessable("backup.schedule_invalid", error.to_string())
            .with_param("value", expression.chars().take(64).collect::<String>())
    })?;
    let verify_expression = request
        .verify_schedule
        .as_deref()
        .map(str::trim)
        .filter(|expression| !expression.is_empty())
        .map(str::to_owned);
    let verify_next = match &verify_expression {
        Some(verify) => Some(schedule::next_run(verify, &timezone, now).map_err(|error| {
            ApiError::unprocessable("backup.verify_schedule_invalid", error.to_string())
                .with_param("value", verify.chars().take(64).collect::<String>())
        })?),
        None => None,
    };
    if request.enabled {
        if current.key.is_none() {
            return Err(ApiError::conflict(
                "backup.key_missing",
                "Set up a backup passphrase before switching the schedule on",
            ));
        }
        if !current.destinations.iter().any(|record| record.enabled) {
            return Err(ApiError::bad_request(
                "backup.destination_missing",
                "Add a destination for the backups before switching the schedule on",
            ));
        }
    }
    // A changed schedule, zone or switch is timed again from now; an unchanged one keeps its
    // due time, so saving the destination alone never moves the next run.
    let unchanged = current.enabled == request.enabled
        && current.schedule == expression
        && current.timezone == timezone;
    let next_run_at = match (request.enabled, unchanged) {
        (false, _) => None,
        (true, true) => current.next_run_at.or(Some(next)),
        (true, false) => Some(next),
    };
    // The same rule for the verification schedule.
    let verify_unchanged =
        current.verify_schedule == verify_expression && current.timezone == timezone;
    let verify_next_run_at = match (verify_next, verify_unchanged) {
        (None, _) => None,
        (Some(next), true) => current.verify_next_run_at.or(Some(next)),
        (Some(next), false) => Some(next),
    };
    state
        .database
        .save_backup_config(BackupConfigUpdate {
            enabled: request.enabled,
            schedule: expression.clone(),
            timezone: timezone.clone(),
            next_run_at,
            verify_schedule: verify_expression.clone(),
            verify_next_run_at,
        })
        .await?;
    let mut fields = Vec::new();
    if current.enabled != request.enabled {
        fields.push("enabled");
    }
    if current.schedule != expression {
        fields.push("schedule");
    }
    if current.timezone != timezone {
        fields.push("timezone");
    }
    if current.verify_schedule != verify_expression {
        fields.push("verify_schedule");
    }
    crate::audit::record(
        &state,
        AuditEvent::success(AuditAction::BackupConfigured)
            .by(&audit)
            .target("backup", "full_backup")
            .detail("fields", fields.join(" "))
            .detail("enabled", request.enabled),
    )
    .await;
    Ok(Json(config_response(&state).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/backups/passphrase",
    tag = "system",
    request_body = SetBackupPassphraseRequest,
    responses((status = 200, body = BackupConfigResponse), (status = 400, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody))
)]
pub async fn set_backup_passphrase(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<SetBackupPassphraseRequest>,
) -> Result<Json<BackupConfigResponse>, ApiError> {
    if request.passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(ApiError::bad_request(
            "backup.passphrase_too_short",
            format!("The backup passphrase needs at least {MIN_PASSPHRASE_CHARS} characters"),
        )
        .with_param("min", MIN_PASSPHRASE_CHARS));
    }
    let current = state.database.backup_config().await?.key;
    if let Some(stored) = current {
        verify_current_passphrase(
            &state,
            &audit,
            &stored,
            request.current_passphrase.as_deref(),
        )
        .await?;
    }
    let key = BackupKey::derive_new(&request.passphrase).await?;
    drop(request);
    let reference = state.secrets.put_bytes(key.key_bytes()).await?;
    let fingerprint = key.fingerprint();
    let record = BackupKeyRecord {
        reference: reference.clone(),
        salt: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key.salt()),
        fingerprint: fingerprint.clone(),
        set_at: Utc::now(),
    };
    let previous = match state.database.set_backup_key(record).await {
        Ok(previous) => previous,
        Err(error) => {
            // The new key never became the configured one; it must not linger in the store.
            if let Err(cleanup) = state.secrets.remove(&reference).await {
                tracing::warn!(%cleanup, "an unused backup key could not be removed");
            }
            return Err(error.into());
        }
    };
    if let Some(previous) = previous
        && let Err(error) = state.secrets.remove(&previous).await
    {
        tracing::warn!(%error, "the replaced backup key could not be removed");
    }
    crate::audit::record(
        &state,
        AuditEvent::success(AuditAction::BackupKeyChanged)
            .by(&audit)
            .target("backup", "full_backup")
            .detail("fingerprint", &fingerprint),
    )
    .await;
    Ok(Json(config_response(&state).await?))
}

/// Replacing a passphrase asks for the one in force (owner's decision, 2026-09-28): whoever
/// holds a session must not be able to seal every future backup under a passphrase only they
/// know. The current passphrase is derived under the stored salt and compared with the stored
/// key in constant time; when the secret store cannot hand the key back, with its fingerprint.
/// A wrong one is audited as a failure — without either passphrase.
async fn verify_current_passphrase(
    state: &AppState,
    audit: &AuditContext,
    stored: &BackupKeyRecord,
    current: Option<&str>,
) -> Result<(), ApiError> {
    let Some(current) = current.filter(|value| !value.is_empty()) else {
        return Err(ApiError::bad_request(
            "backup.passphrase_current_required",
            "Enter the current backup passphrase to replace it",
        ));
    };
    let salt = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &stored.salt)
        .map_err(|error| anyhow::anyhow!("stored backup salt: {error}"))?;
    let salt: [u8; rd_backup::crypto::SALT_LEN] = salt
        .try_into()
        .map_err(|_| anyhow::anyhow!("the stored backup salt has the wrong length"))?;
    let candidate = BackupKey::derive(current, salt).await?;
    let matches = match state.secrets.get_bytes(&stored.reference).await {
        Ok(bytes) => BackupKey::from_stored(&bytes, &salt)
            .map(|stored_key| stored_key.matches(&candidate))
            .unwrap_or_else(|_| candidate.has_fingerprint(&stored.fingerprint)),
        Err(error) => {
            tracing::warn!(%error, "the stored backup key could not be read; checking its fingerprint");
            candidate.has_fingerprint(&stored.fingerprint)
        }
    };
    if matches {
        return Ok(());
    }
    crate::audit::record(
        state,
        AuditEvent::failure(AuditAction::BackupKeyChanged)
            .by(audit)
            .target("backup", "full_backup")
            .detail("reason", "current_passphrase_wrong"),
    )
    .await;
    Err(ApiError::forbidden(
        "backup.passphrase_wrong",
        "The current backup passphrase is wrong",
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/backups/runs",
    tag = "system",
    responses((status = 202, body = BackupRunResponse), (status = 409, body = crate::error::ErrorBody))
)]
pub async fn run_backup(
    State(state): State<AppState>,
    audit: AuditContext,
) -> Result<(StatusCode, Json<BackupRunResponse>), ApiError> {
    let run = crate::backup_service::start_run(&state, BackupOrigin::Manual, audit).await?;
    Ok((StatusCode::ACCEPTED, Json(run.into())))
}

#[utoipa::path(get, path = "/api/v1/backups/runs", tag = "system", responses((status = 200, body = [BackupRunResponse])))]
pub async fn list_backup_runs(
    State(state): State<AppState>,
) -> Result<Json<Vec<BackupRunResponse>>, ApiError> {
    Ok(Json(
        state
            .database
            .backup_runs(RUNS_LISTED)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

//! Verifying archives at their destinations (RD-160-02): by hand for one archive, and on the
//! verification schedule for the newest archive of every destination.
//!
//! A verification fetches the archive into its own folder below the data directory
//! (`backup-verify`, not the run's staging folder, which a finishing run empties), compares it
//! with the ledger and opens it with the backup key (`rd_backup::verify`). It reads a
//! destination and nothing else, so it can run beside a backup run and never touches a
//! download; its result lands in the verification history, on the archive's ledger row, in the
//! log and in the audit record.

use std::path::PathBuf;

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rd_backup::{
    BackupKey,
    schedule::Due,
    verify::{ExpectedArchive, verify_at},
};
use rd_core::{AuditAction, BackupOrigin, BackupVerifyState};
use rd_db::{BackupArchive, BackupVerification, BackupVerificationOutcome};

use crate::audit::{AuditContext, AuditEvent};
use crate::{ApiError, AppState, backup_delivery};

/// The folder archives are fetched into for a check, below the data directory.
pub const VERIFY_DIR: &str = "backup-verify";

/// Where verifications fetch their copies.
#[must_use]
pub fn scratch_root(state: &AppState) -> PathBuf {
    crate::backup_service::data_directory(state).join(VERIFY_DIR)
}

/// One look at the verification schedule at `now`; returns how many verifications it started.
pub async fn tick(state: &AppState, now: DateTime<Utc>) -> usize {
    let config = match state.database.backup_config().await {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "the backup verification schedule could not be read");
            return 0;
        }
    };
    let Some(expression) = config.verify_schedule.clone() else {
        return 0;
    };
    let decision = rd_backup::schedule::decide(
        true,
        &expression,
        &config.timezone,
        config.verify_next_run_at,
        now,
    );
    match decision {
        Ok(Due::Idle | Due::Wait) => 0,
        Ok(Due::Arm(next)) => {
            if let Err(error) = state.database.arm_backup_verify(Some(next)).await {
                tracing::warn!(%error, "the backup verification schedule could not be armed");
            }
            0
        }
        Ok(Due::Run { next }) => {
            // Advanced first, for the reason the backup schedule is (`backup_service::tick`).
            if let Err(error) = state.database.arm_backup_verify(Some(next)).await {
                tracing::warn!(%error, "the backup verification schedule could not be advanced");
                return 0;
            }
            let mut started = 0;
            for destination in config.destinations.iter().filter(|record| record.enabled) {
                let newest = match state.database.backup_archives(Some(&destination.id)).await {
                    Ok(archives) => archives.into_iter().next(),
                    Err(error) => {
                        tracing::warn!(%error, "the backup ledger could not be read");
                        None
                    }
                };
                let Some(newest) = newest else {
                    continue;
                };
                let system = AuditContext {
                    actor: crate::audit::Actor::system(),
                    trace: None,
                };
                match start_verification(state, &newest, BackupOrigin::Scheduled, system).await {
                    Ok(_) => started += 1,
                    Err(error) => tracing::warn!(
                        code = error.code(),
                        error = error.message(),
                        "a scheduled backup verification did not start"
                    ),
                }
            }
            started
        }
        Err(error) => {
            tracing::warn!(%error, code = "backup.verify_schedule_invalid", "the verification schedule names no time");
            0
        }
    }
}

/// Records a verification of `archive` and runs it in the background; returns its row.
///
/// # Errors
///
/// `backup.key_missing` when no passphrase is set up, `backup.destination_missing` when the
/// archive's destination is gone.
pub async fn start_verification(
    state: &AppState,
    archive: &BackupArchive,
    origin: BackupOrigin,
    audit: AuditContext,
) -> Result<BackupVerification, ApiError> {
    let config = state.database.backup_config().await?;
    let Some(key_record) = config.key else {
        return Err(ApiError::conflict(
            "backup.key_missing",
            "Set up a backup passphrase before verifying archives",
        ));
    };
    let Some(record) = state
        .database
        .backup_destination(&archive.destination_id)
        .await?
    else {
        return Err(ApiError::not_found(
            "backup.destination_missing",
            "The archive's destination no longer exists",
        ));
    };
    let verification = BackupVerification {
        id: uuid::Uuid::now_v7().to_string(),
        origin,
        state: BackupVerifyState::Running,
        archive_id: Some(archive.id.clone()),
        destination_id: Some(record.id.clone()),
        destination: backup_delivery::label(&record),
        archive_name: archive.archive_name.clone(),
        started_at: Utc::now(),
        finished_at: None,
        content_checked: None,
        error_code: None,
        error_detail: None,
    };
    state
        .database
        .begin_backup_verification(verification.clone())
        .await?;
    let task_state = state.clone();
    let id = verification.id.clone();
    let archive = archive.clone();
    tokio::spawn(async move {
        let outcome = check(&task_state, &record, &archive, &key_record).await;
        let (stored, event) = match outcome {
            Ok(content_checked) => {
                tracing::info!(archive = %archive.archive_name, content_checked, "backup archive verified");
                (
                    BackupVerificationOutcome::Passed { content_checked },
                    AuditEvent::success(AuditAction::BackupVerified)
                        .detail("content_checked", content_checked),
                )
            }
            Err(error) => {
                tracing::warn!(
                    archive = %archive.archive_name,
                    code = error.code,
                    detail = %error.detail,
                    "backup archive verification failed"
                );
                (
                    BackupVerificationOutcome::Failed {
                        code: error.code.to_owned(),
                        detail: error.detail,
                    },
                    AuditEvent::failure(AuditAction::BackupVerified).detail("code", error.code),
                )
            }
        };
        if let Err(error) = task_state
            .database
            .finish_backup_verification(id.clone(), stored)
            .await
        {
            tracing::warn!(%error, "the end of a backup verification could not be recorded");
        }
        crate::audit::record(
            &task_state,
            event
                .by(&audit)
                .target("backup_archive", &archive.id)
                .detail("archive", &archive.archive_name)
                .detail("origin", origin.as_str()),
        )
        .await;
    });
    Ok(verification)
}

async fn check(
    state: &AppState,
    record: &rd_db::BackupDestinationRecord,
    archive: &BackupArchive,
    key_record: &rd_db::BackupKeyRecord,
) -> Result<bool, rd_backup::BackupError> {
    let unavailable = |detail: String| rd_backup::BackupError {
        code: "backup.key_unavailable",
        detail,
    };
    let key_bytes = state
        .secrets
        .get_bytes(&key_record.reference)
        .await
        .map_err(|error| unavailable(format!("{error:#}")))?;
    let salt = STANDARD
        .decode(&key_record.salt)
        .map_err(|error| unavailable(error.to_string()))?;
    let key = BackupKey::from_stored(&key_bytes, &salt)
        .map_err(|error| unavailable(format!("{error:#}")))?;
    let context = backup_delivery::destination_context(state).await;
    let destination = backup_delivery::open(&context, record)
        .await
        .map_err(|error| rd_backup::BackupError {
            code: error.code(),
            detail: error.to_string(),
        })?;
    let verified = verify_at(
        destination.as_ref(),
        &ExpectedArchive {
            name: archive.archive_name.clone(),
            size_bytes: archive.size_bytes,
            sha256: archive.sha256.clone(),
        },
        &key,
        &scratch_root(state),
    )
    .await?;
    Ok(verified.content_checked)
}

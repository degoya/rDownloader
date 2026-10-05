//! Scheduled and manual full backups (RD-160-01): the loop, one run, and the start's recovery.
//!
//! A run is its own task. It reads the live database once — the snapshot, a writer command —
//! and otherwise only its own tables, so nothing it does can hold up, pause or fail a download:
//! a failure ends the run, lands in the history with its stable code, is logged and audited,
//! and that is all. At most one run exists at a time, which the store enforces
//! (`backup_runs_one_running_idx`), so a manual run and a scheduled one cannot overlap.
//!
//! Every enabled destination gets its own copy of the archive (RD-160-02,
//! `crate::backup_delivery`): a run succeeds when at least one destination has it, and each
//! destination's outcome is its own row. The same loop starts the scheduled verification
//! (`crate::backup_verify_service`).
//!
//! The due time is stored before a scheduled run starts (see `rd_backup::schedule`), and the
//! start marks a run that was still `running` as interrupted and empties the staging folder.

use std::path::PathBuf;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rd_api_core::notify_notice::Notice;
use rd_backup::{BackupError, BackupKey, BackupSources, RetryPolicy, schedule::Due};
use rd_core::{AuditAction, BackupOrigin};
use rd_db::{BackupConfig, BackupRunOutcome, NewBackupRun};

use crate::audit::{AuditContext, AuditEvent};
use crate::backup_delivery::{self, Delivered, Target};
use crate::settings_backup::{SecretSealing, build_settings_bundle};
use crate::{ApiError, AppState};

mod produce;

pub(crate) use produce::*;

/// How often the loop looks at the schedule. Cron has minutes; half of one is close enough.
const TICK: Duration = Duration::from_secs(30);

/// Starts the loop. Ends with the application state, like the other supervisors.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        recover(&state).await;
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now = Utc::now();
            tick(&state, now).await;
            crate::backup_verify_service::tick(&state, now).await;
        }
    });
}

/// The data directory: the folder the database lives in.
pub(crate) fn data_directory(state: &AppState) -> PathBuf {
    state
        .database
        .path()
        .parent()
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
}

/// What the start does before the first tick: a run still `running` belongs to a process that
/// is gone, and whatever it staged is no archive anybody has.
pub async fn recover(state: &AppState) {
    match state.database.interrupt_backup_runs().await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(
            count,
            code = rd_db::BACKUP_INTERRUPTED,
            "a backup was interrupted by the last stop"
        ),
        Err(error) => tracing::warn!(%error, "interrupted backups could not be recorded"),
    }
    if let Err(error) =
        rd_backup::sweep_staging(&rd_backup::staging_root(&data_directory(state))).await
    {
        tracing::warn!(%error, "the backup staging folder could not be emptied");
    }
    if let Err(error) =
        rd_backup::sweep_staging(&crate::backup_verify_service::scratch_root(state)).await
    {
        tracing::warn!(%error, "the backup verification folder could not be emptied");
    }
    // What a preparation for an update left when the process stopped in it (RD-180-03).
    if let Err(error) = rd_backup::pre_update::sweep(&data_directory(state)).await {
        tracing::warn!(%error, "the pre-update folder could not be swept");
    }
}

/// One look at the schedule at `now`; returns the id of the run it started, if any.
pub async fn tick(state: &AppState, now: DateTime<Utc>) -> Option<String> {
    let config = match state.database.backup_config().await {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "the backup schedule could not be read");
            return None;
        }
    };
    let decision = rd_backup::schedule::decide(
        config.enabled,
        &config.schedule,
        &config.timezone,
        config.next_run_at,
        now,
    );
    match decision {
        Ok(Due::Idle | Due::Wait) => None,
        Ok(Due::Arm(next)) => {
            if let Err(error) = state.database.arm_backup(Some(next)).await {
                tracing::warn!(%error, "the backup schedule could not be armed");
            }
            None
        }
        Ok(Due::Run { next }) => {
            // The next due time first: a run that crashes the process must not be started
            // again by every start that follows, and a due time that cannot be stored must not
            // start a run on every tick.
            if let Err(error) = state.database.arm_backup(Some(next)).await {
                tracing::warn!(%error, "the backup schedule could not be advanced");
                return None;
            }
            let system = AuditContext {
                actor: crate::audit::Actor::system(),
                trace: None,
            };
            match start_run(state, BackupOrigin::Scheduled, system).await {
                Ok(run) => Some(run.id),
                Err(error) => {
                    tracing::warn!(
                        code = error.code(),
                        error = error.message(),
                        "the scheduled backup did not start"
                    );
                    None
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, code = "backup.schedule_invalid", "the backup schedule names no time");
            None
        }
    }
}

/// Records a run and starts it in the background; returns its row.
///
/// # Errors
///
/// `backup.already_running` when another run has not finished.
pub async fn start_run(
    state: &AppState,
    origin: BackupOrigin,
    audit: AuditContext,
) -> Result<rd_db::BackupRun, ApiError> {
    let config = state.database.backup_config().await?;
    let run_id = uuid::Uuid::now_v7().to_string();
    let started_at = Utc::now();
    let destinations: Vec<_> = config
        .destinations
        .iter()
        .filter(|record| record.enabled)
        .collect();
    // The run's own destination columns name the destinations in one line; each has its row.
    let names = destinations
        .iter()
        .map(|record| backup_delivery::label(record))
        .collect::<Vec<_>>()
        .join(", ");
    let begun = state
        .database
        .begin_backup_run(NewBackupRun {
            id: run_id.clone(),
            origin,
            started_at,
            destination_id: (destinations.len() == 1).then(|| destinations[0].id.clone()),
            destination: (!names.is_empty()).then_some(names),
        })
        .await?;
    if !begun {
        return Err(ApiError::conflict(
            "backup.already_running",
            "A backup is already running",
        ));
    }
    state
        .database
        .begin_backup_run_destinations(
            run_id.clone(),
            destinations
                .iter()
                .map(|record| {
                    (
                        record.id.clone(),
                        record.kind.clone(),
                        backup_delivery::label(record),
                    )
                })
                .collect(),
        )
        .await?;
    let run = state
        .database
        .backup_run(&run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the backup run just recorded is gone"))?;
    tokio::spawn(execute(
        state.clone(),
        config,
        run_id,
        origin,
        started_at,
        audit,
    ));
    Ok(run)
}

/// The run itself, then its row, the staging folder, the log and the audit record.
async fn execute(
    state: AppState,
    config: BackupConfig,
    run_id: String,
    origin: BackupOrigin,
    started_at: DateTime<Utc>,
    audit: AuditContext,
) {
    let destinations: Vec<_> = config
        .destinations
        .iter()
        .filter(|record| record.enabled)
        .map(|record| record.id.clone())
        .collect();
    let result = produce(&state, config, &run_id, started_at).await;
    if let Err(error) =
        rd_backup::sweep_staging(&rd_backup::staging_root(&data_directory(&state))).await
    {
        tracing::warn!(%error, "the backup staging folder could not be emptied");
    }
    if let Err(error) = &result {
        // Nothing was delivered anywhere: a destination whose row is still open ends with the
        // run's reason (one already ended keeps its own).
        for destination_id in destinations {
            finish_failed(
                &state,
                &run_id,
                &destination_id,
                error.code,
                error.detail.clone(),
            )
            .await;
        }
    }
    // Nobody watches a scheduled run; a manual one shows its outcome to whoever started it.
    let notice = match &result {
        _ if origin != BackupOrigin::Scheduled => None,
        Ok(written) if written.failed.is_empty() => None,
        Ok(written) => Some(Notice::backup_partial(&run_id, &written.failed)),
        Err(error) => Some(Notice::backup_failed(&run_id, error.code, &error.detail)),
    };
    let (outcome, event) = match result {
        Ok(written) => {
            tracing::info!(
                archive = %written.archive_name,
                size = written.size_bytes,
                failed = written.failed.len(),
                "backup written"
            );
            let mut event = AuditEvent::success(AuditAction::BackupCreated)
                .detail("archive", &written.archive_name)
                .detail("size_bytes", written.size_bytes)
                .detail("destinations", written.delivered);
            let (error_code, error_detail) = if written.failed.is_empty() {
                (None, None)
            } else {
                event = event.detail("failed_destinations", written.failed.len());
                (
                    Some("backup.destinations_partial".to_owned()),
                    Some(written.failed.join("; ")),
                )
            };
            (
                BackupRunOutcome::Succeeded {
                    parts: serde_json::to_value(&written.parts).unwrap_or_default(),
                    archive_name: written.archive_name,
                    size_bytes: written.size_bytes,
                    sha256: written.sha256,
                    error_code,
                    error_detail,
                },
                event,
            )
        }
        Err(error) => {
            tracing::warn!(code = error.code, detail = %error.detail, "backup failed");
            let event = AuditEvent::failure(AuditAction::BackupCreated).detail("code", error.code);
            (
                BackupRunOutcome::Failed {
                    code: error.code.to_owned(),
                    detail: error.detail,
                },
                event,
            )
        }
    };
    if let Err(error) = state
        .database
        .finish_backup_run(run_id.clone(), outcome)
        .await
    {
        tracing::warn!(%error, "the end of a backup run could not be recorded");
    }
    if let Some(notice) = notice {
        rd_api_core::notify_notice::announce(&state.database, notice).await;
    }
    crate::audit::record(
        &state,
        event
            .by(&audit)
            .target("backup_run", &run_id)
            .detail("origin", origin.as_str()),
    )
    .await;
}

/// Ends one destination's row of a run that delivered nothing to it.
async fn finish_failed(
    state: &AppState,
    run_id: &str,
    destination_id: &str,
    code: &str,
    detail: String,
) {
    if let Err(error) = state
        .database
        .finish_backup_run_destination(rd_db::BackupRunDestinationEnd {
            run_id: run_id.to_owned(),
            destination_id: destination_id.to_owned(),
            state: rd_core::BackupRunState::Failed,
            attempts: 0,
            location: None,
            pruned: 0,
            error_code: Some(code.to_owned()),
            error_detail: Some(detail),
        })
        .await
    {
        tracing::warn!(%error, "the end of a backup destination could not be recorded");
    }
}

fn failure(code: &'static str, detail: impl Into<String>) -> BackupError {
    BackupError {
        code,
        detail: detail.into(),
    }
}

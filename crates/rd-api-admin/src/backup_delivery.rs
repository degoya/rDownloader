//! A sealed archive on its way to every destination, and retention behind it (RD-160-02).
//!
//! Each destination is opened, delivered to and pruned on its own; one that is down costs its
//! own row in the history and nothing else. Retention runs only after the new archive is at
//! that destination and only over the ledger's archives of this installation, so a destination
//! that failed keeps every archive it had.

use chrono::Utc;
use rd_backup::{
    BackupDestination, DestinationConfig, DestinationContext, DestinationError, RecordedArchive,
    RetentionPolicy, RetryPolicy, SealedBackup, retention,
};
use rd_core::BackupRunState;
use rd_db::{BackupDestinationRecord, BackupRunDestinationEnd, NewBackupArchive};

use crate::AppState;

/// What opening a destination needs, read from the live settings.
pub async fn destination_context(state: &AppState) -> DestinationContext {
    let postprocess = match state
        .database
        .service_settings::<rd_core::PostprocessSettings>()
        .await
    {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "the post-processing settings could not be read; rclone is looked up on PATH");
            rd_core::PostprocessSettings::default()
        }
    };
    DestinationContext {
        object_storage: state.object_storage.clone(),
        rclone_executable: postprocess.rclone_executable,
        vendor_directory: postprocess.vendor_directory,
        bandwidth: state.scheduler.bandwidth().upload_limiter(),
    }
}

/// How the history names a destination row.
#[must_use]
pub fn label(record: &BackupDestinationRecord) -> String {
    if record.name.trim().is_empty() {
        DestinationConfig::parse(&record.kind, &record.config)
            .map_or_else(|| record.kind.clone(), |config| config.describe())
    } else {
        record.name.clone()
    }
}

/// Opens a stored destination for a run or a verification.
///
/// # Errors
///
/// When the row names no destination this build knows, or it cannot serve.
pub async fn open(
    context: &DestinationContext,
    record: &BackupDestinationRecord,
) -> Result<Box<dyn BackupDestination>, DestinationError> {
    let Some(config) = DestinationConfig::parse(&record.kind, &record.config) else {
        return Err(DestinationError::Misconfigured {
            code: rd_backup::remote::DESTINATION_KIND_UNKNOWN,
            detail: format!("destination kind {} is not supported", record.kind),
        });
    };
    config.open(context).await
}

/// One destination's share of a run: its row, what opening it gave.
pub struct Target {
    pub record: BackupDestinationRecord,
    pub opened: Result<Box<dyn BackupDestination>, DestinationError>,
}

/// How one destination's delivery ended, for the run's own row.
pub struct Delivered {
    pub label: String,
    pub result: Result<(), (&'static str, String)>,
}

/// Delivers `sealed` to every opened target at once, records each archive in the ledger,
/// prunes behind it and finishes each destination's row.
pub async fn deliver(
    state: &AppState,
    run_id: &str,
    instance_id: &str,
    sealed: &SealedBackup,
    created_at: chrono::DateTime<Utc>,
    targets: Vec<Target>,
    policy: RetryPolicy,
) -> Vec<Delivered> {
    futures_util::future::join_all(targets.into_iter().map(|target| async move {
        let label = label(&target.record);
        let result = match &target.opened {
            Ok(destination) => {
                let delivery = rd_backup::deliver(
                    destination.as_ref(),
                    &sealed.path,
                    &sealed.archive_name,
                    policy,
                )
                .await;
                (delivery.attempts, delivery.result)
            }
            Err(error) => (
                0,
                Err(DestinationError::Misconfigured {
                    code: error.code(),
                    detail: error.to_string(),
                }),
            ),
        };
        let (attempts, stored) = result;
        let end = match stored {
            Ok(stored) => {
                let recorded = state
                    .database
                    .record_backup_archive(NewBackupArchive {
                        destination_id: target.record.id.clone(),
                        run_id: run_id.to_owned(),
                        archive_name: sealed.archive_name.clone(),
                        location: stored.location.clone(),
                        size_bytes: sealed.size_bytes,
                        sha256: sealed.sha256.clone(),
                        created_at,
                    })
                    .await;
                if let Err(error) = recorded {
                    tracing::warn!(%error, "a stored backup archive could not be recorded");
                }
                let pruned = match &target.opened {
                    Ok(destination) => {
                        prune(state, destination.as_ref(), &target.record, instance_id).await
                    }
                    Err(_) => 0,
                };
                (
                    BackupRunDestinationEnd {
                        run_id: run_id.to_owned(),
                        destination_id: target.record.id.clone(),
                        state: BackupRunState::Succeeded,
                        attempts,
                        location: Some(stored.location),
                        pruned,
                        error_code: None,
                        error_detail: None,
                    },
                    Ok(()),
                )
            }
            Err(error) => {
                tracing::warn!(
                    destination = %label,
                    code = error.code(),
                    %error,
                    "a backup destination did not get the archive"
                );
                (
                    BackupRunDestinationEnd {
                        run_id: run_id.to_owned(),
                        destination_id: target.record.id.clone(),
                        state: BackupRunState::Failed,
                        attempts,
                        location: None,
                        pruned: 0,
                        error_code: Some(error.code().to_owned()),
                        error_detail: Some(error.to_string()),
                    },
                    Err((error.code(), error.to_string())),
                )
            }
        };
        let (row, result) = end;
        if let Err(error) = state.database.finish_backup_run_destination(row).await {
            tracing::warn!(%error, "the end of a backup destination could not be recorded");
        }
        Delivered { label, result }
    }))
    .await
}

/// The retention pass of one destination after a successful delivery; returns how many
/// archives it removed. A removal that fails leaves the archive in the ledger for the next
/// pass, and is logged.
pub async fn prune(
    state: &AppState,
    destination: &dyn BackupDestination,
    record: &BackupDestinationRecord,
    instance_id: &str,
) -> u32 {
    let policy = RetentionPolicy {
        keep_last: record.keep_last,
        keep_days: record.keep_days,
    };
    if policy.is_unlimited() {
        return 0;
    }
    let ledger = match state.database.backup_archives(Some(&record.id)).await {
        Ok(ledger) => ledger,
        Err(error) => {
            tracing::warn!(%error, "the backup ledger could not be read; nothing is pruned");
            return 0;
        }
    };
    let recorded: Vec<RecordedArchive> = ledger
        .iter()
        .map(|archive| RecordedArchive {
            id: archive.id.clone(),
            name: archive.archive_name.clone(),
            created_at: archive.created_at,
        })
        .collect();
    let plan = retention::plan(&recorded, policy, instance_id, Utc::now());
    let mut forgotten = Vec::new();
    for id in plan.remove {
        let Some(archive) = ledger.iter().find(|archive| archive.id == id) else {
            continue;
        };
        match destination.remove(&archive.archive_name).await {
            // Already gone is what retention wanted.
            Ok(()) | Err(DestinationError::NotFound(_)) => forgotten.push(id),
            Err(error) => tracing::warn!(
                archive = %archive.archive_name,
                code = error.code(),
                %error,
                "retention could not remove an old backup archive"
            ),
        }
    }
    if forgotten.is_empty() {
        return 0;
    }
    match state.database.forget_backup_archives(forgotten).await {
        Ok(count) => u32::try_from(count).unwrap_or(u32::MAX),
        Err(error) => {
            tracing::warn!(%error, "removed backup archives could not be forgotten");
            0
        }
    }
}

//! A sealed archive on its way to every destination, recorded in the ledger, and retention
//! behind it (RD-160-02).
//!
//! Each destination is delivered to and pruned on its own; one that is down costs its own row in
//! the history and nothing else. Retention runs only after the new archive is at that
//! destination and only over the ledger's archives of this installation, so a destination that
//! failed keeps every archive it had.
//!
//! The service called this from `rd-api-admin`; it lives here so its two crash points sit in
//! the crate that owns them (RD-170-07, recovery matrix): `backup.before_archive_recorded`, an
//! archive at its destination the ledger does not know yet, and `backup.after_retention_removal`,
//! an archive removed there that the ledger still lists.

use chrono::{DateTime, Utc};
use rd_core::BackupRunState;
use rd_db::{BackupDestinationRecord, BackupRunDestinationEnd, Database, NewBackupArchive};

use crate::create::SealedBackup;
use crate::deliver::RetryPolicy;
use crate::destination::{BackupDestination, DestinationError, StoredBackup};
use crate::remote::DestinationConfig;
use crate::retention::{self, RecordedArchive, RetentionPolicy};

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

/// What one delivery is about.
#[derive(Clone, Copy)]
pub struct Run<'a> {
    pub id: &'a str,
    /// This installation's id, which retention tells its own archives by.
    pub instance_id: &'a str,
    pub sealed: &'a SealedBackup,
    pub created_at: DateTime<Utc>,
}

/// Delivers the sealed archive to every opened target at once, records each archive in the
/// ledger, prunes behind it and finishes each destination's row.
pub async fn deliver(
    database: &Database,
    run: Run<'_>,
    targets: Vec<Target>,
    policy: RetryPolicy,
) -> Vec<Delivered> {
    let outcomes = futures_util::future::join_all(
        targets
            .into_iter()
            .map(|target| deliver_one(database, run, target, policy)),
    )
    .await;
    // Only a crash point ends a destination's share without an outcome: the process stops
    // there, and its row stays `running` for the next start to mark interrupted.
    outcomes.into_iter().filter_map(Result::ok).collect()
}

async fn deliver_one(
    database: &Database,
    run: Run<'_>,
    target: Target,
    policy: RetryPolicy,
) -> anyhow::Result<Delivered> {
    let label = label(&target.record);
    let (attempts, stored) = match &target.opened {
        Ok(destination) => {
            let delivery = crate::deliver::deliver(
                destination.as_ref(),
                &run.sealed.path,
                &run.sealed.archive_name,
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
    let (row, result) = match stored {
        Ok(stored) => {
            record(database, run, &target.record, &stored).await?;
            let pruned = match &target.opened {
                Ok(destination) => {
                    prune(
                        database,
                        destination.as_ref(),
                        &target.record,
                        run.instance_id,
                    )
                    .await?
                }
                Err(_) => 0,
            };
            (
                BackupRunDestinationEnd {
                    run_id: run.id.to_owned(),
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
                    run_id: run.id.to_owned(),
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
    if let Err(error) = database.finish_backup_run_destination(row).await {
        tracing::warn!(%error, "the end of a backup destination could not be recorded");
    }
    Ok(Delivered { label, result })
}

/// Records an archive its destination has. A failure to record is logged: the archive stands
/// and is still listed by the destination, only retention never removes it.
///
/// # Errors
///
/// Only at the crash point.
async fn record(
    database: &Database,
    run: Run<'_>,
    destination: &BackupDestinationRecord,
    stored: &StoredBackup,
) -> anyhow::Result<()> {
    rd_core::failpoint!("backup.before_archive_recorded", || anyhow::anyhow!(
        "crash point"
    ));
    let recorded = database
        .record_backup_archive(NewBackupArchive {
            destination_id: destination.id.clone(),
            run_id: run.id.to_owned(),
            archive_name: run.sealed.archive_name.clone(),
            location: stored.location.clone(),
            size_bytes: run.sealed.size_bytes,
            sha256: run.sealed.sha256.clone(),
            created_at: run.created_at,
        })
        .await;
    if let Err(error) = recorded {
        tracing::warn!(%error, "a stored backup archive could not be recorded");
    }
    Ok(())
}

/// The retention pass of one destination after a successful delivery; returns how many
/// archives it removed. A removal that fails leaves the archive in the ledger for the next
/// pass, and is logged; so is a ledger that cannot be read or written.
///
/// The ledger forgets only what the destination no longer has: an archive is removed there
/// first, and one a stopped pass removed without forgetting is found gone — `NotFound` — and
/// forgotten by the next.
///
/// # Errors
///
/// Only at the crash point.
pub async fn prune(
    database: &Database,
    destination: &dyn BackupDestination,
    record: &BackupDestinationRecord,
    instance_id: &str,
) -> anyhow::Result<u32> {
    let policy = RetentionPolicy {
        keep_last: record.keep_last,
        keep_days: record.keep_days,
    };
    if policy.is_unlimited() {
        return Ok(0);
    }
    let ledger = match database.backup_archives(Some(&record.id)).await {
        Ok(ledger) => ledger,
        Err(error) => {
            tracing::warn!(%error, "the backup ledger could not be read; nothing is pruned");
            return Ok(0);
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
            Ok(()) | Err(DestinationError::NotFound(_)) => {
                forgotten.push(id);
                rd_core::failpoint!("backup.after_retention_removal", || anyhow::anyhow!(
                    "crash point"
                ));
            }
            Err(error) => tracing::warn!(
                archive = %archive.archive_name,
                code = error.code(),
                %error,
                "retention could not remove an old backup archive"
            ),
        }
    }
    if forgotten.is_empty() {
        return Ok(0);
    }
    match database.forget_backup_archives(forgotten).await {
        Ok(count) => Ok(u32::try_from(count).unwrap_or(u32::MAX)),
        Err(error) => {
            tracing::warn!(%error, "removed backup archives could not be forgotten");
            Ok(0)
        }
    }
}

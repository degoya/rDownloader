//! Preview, test restore and restore of a full backup (RD-160-03).
//!
//! Every step derives the key from the passphrase in the request and the salt in the archive's
//! header; the key the service keeps for its scheduled runs is never read here (owner's
//! decision, 2026-09-28). A wrong passphrase is audited as a failure, without the passphrase.
//!
//! A test restore and a restore run the same checks on an unpacked copy below
//! `restore-work/`: the copy is migrated, its paths moved by the requested mappings, its
//! credentials matched against the sealed settings bundle, its references and torrent files
//! checked. The test restore then removes the copy; nothing it did reached the secret store or
//! the live database. The restore puts the credentials into the secret store, writes their new
//! references into the copy and stages it; the switch is the next start's
//! (`rd_backup::restore::cutover`). One restore step runs at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use rd_backup::manifest::{PartKind, SETTINGS_PART, TORRENT_FILES_PREFIX};
use rd_backup::restore::cutover::{self, Layout, PendingRestore, Phase};
use rd_backup::restore::plan::{self, PathFindings, RequestedMapping};
use rd_backup::restore::{RestoreError, inspect, paths};
use rd_backup::{BackupKey, manifest::DATABASE_PART};
use rd_core::AuditAction;
use rd_db::restore_copy::{self, CopyUpdate};

use crate::audit::{AuditContext, AuditEvent};
use crate::restore_checks::{self, Findings};
use crate::restore_dto::{
    RestoreCountsResponse, RestoreReportResponse, RestoreRequest, RestoreRootPlanResponse,
    RestoreSchemaResponse, RestoreSourceRequest,
};
use crate::settings_backup_dto::SettingsBundle;
use crate::{ApiError, AppState};

mod copy_check;

pub(crate) use copy_check::*;

/// Held while a preview, test restore or restore runs; each unpacks or reads a whole archive.
static BUSY: AtomicBool = AtomicBool::new(false);

pub(crate) struct Busy;

impl Busy {
    pub(crate) fn take() -> Result<Self, ApiError> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                ApiError::conflict(
                    "backup.restore_busy",
                    "Another restore step is running; try again when it finished",
                )
            })
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// The installation's layout: the data directory is the database's folder.
pub(crate) fn layout(state: &AppState) -> Layout {
    Layout::new(state.database.path())
}

/// A restore step's failure as an API error with its own status.
pub(crate) fn api_error(error: RestoreError) -> ApiError {
    match error.code {
        "backup.restore_passphrase_wrong" => ApiError::forbidden(error.code, error.detail),
        "backup.restore_failed" => {
            ApiError::from(anyhow::anyhow!("{}: {}", error.code, error.detail))
        }
        code if code.starts_with("backup.restore_mapping_") => {
            ApiError::unprocessable(code, error.detail)
        }
        code => ApiError::bad_request(code, error.detail),
    }
}

/// The archive a backup run wrote, from one of the destinations that received it: a copy in a
/// local folder is read where it lies, one elsewhere (a bucket, an rclone remote) is fetched
/// into the restore work folder first, where the next attempt from the same run finds it.
async fn run_archive(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    let run =
        state.database.backup_run(id).await?.ok_or_else(|| {
            ApiError::not_found("backup.restore_run_unknown", "No such backup run")
        })?;
    let without_archive = || {
        ApiError::bad_request(
            "backup.restore_run_without_archive",
            "This run wrote no archive",
        )
    };
    let Some(name) = run.archive_name else {
        return Err(without_archive());
    };
    let plain = Path::new(&name)
        .file_name()
        .is_some_and(|file| file == name.as_str());
    if !plain {
        return Err(without_archive());
    }
    let delivered: Vec<_> = run
        .destinations
        .into_iter()
        .filter(|row| row.state == rd_core::BackupRunState::Succeeded)
        .collect();
    for row in &delivered {
        if let Some(location) = row.location.as_deref().filter(|_| row.kind == "local") {
            let path = PathBuf::from(location);
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file())
            {
                return Ok(path);
            }
        }
    }
    let folder = layout(state).work().join(format!("run-{id}"));
    let fetched = folder.join(&name);
    if tokio::fs::metadata(&fetched)
        .await
        .is_ok_and(|metadata| metadata.is_file())
    {
        return Ok(fetched);
    }
    let context = crate::backup_delivery::destination_context(state).await;
    let mut last = None;
    for row in &delivered {
        let Some(record) = state
            .database
            .backup_destination(&row.destination_id)
            .await?
        else {
            continue;
        };
        let destination = match crate::backup_delivery::open(&context, &record).await {
            Ok(destination) => destination,
            Err(error) => {
                last = Some(error.to_string());
                continue;
            }
        };
        tokio::fs::create_dir_all(&folder)
            .await
            .map_err(anyhow::Error::from)?;
        let partial = folder.join(format!("{name}.partial"));
        tokio::fs::remove_file(&partial).await.ok();
        match destination.fetch(&name, &partial).await {
            Ok(_) => {
                tokio::fs::rename(&partial, &fetched)
                    .await
                    .map_err(anyhow::Error::from)?;
                return Ok(fetched);
            }
            Err(error) => {
                tokio::fs::remove_file(&partial).await.ok();
                last = Some(error.to_string());
            }
        }
    }
    Err(ApiError::bad_request(
        "backup.restore_source_missing",
        last.unwrap_or_else(|| "No destination holds this run's archive".to_owned()),
    ))
}

/// The archive a request names.
pub(crate) async fn resolve_source(
    state: &AppState,
    source: &RestoreSourceRequest,
) -> Result<PathBuf, ApiError> {
    let invalid = || {
        ApiError::bad_request(
            "backup.restore_source_invalid",
            "Name exactly one archive: an upload, a path or a run",
        )
    };
    let path = match (&source.upload_id, &source.path, &source.run_id) {
        (Some(id), None, None) => crate::restore_uploads::upload_path(state, id)?,
        (None, Some(path), None) => {
            let path = PathBuf::from(path.trim());
            if !path.is_absolute() {
                return Err(ApiError::bad_request(
                    "backup.restore_source_not_absolute",
                    "The archive path must be absolute",
                ));
            }
            path
        }
        (None, None, Some(id)) => return run_archive(state, id).await,
        _ => return Err(invalid()),
    };
    if !tokio::fs::metadata(&path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
    {
        return Err(ApiError::bad_request(
            "backup.restore_source_missing",
            "The archive is not there",
        ));
    }
    Ok(path)
}

/// Derives the key from the passphrase; a wrong one is audited as a failure.
pub(crate) async fn open(
    state: &AppState,
    audit: &AuditContext,
    archive: &Path,
    passphrase: &str,
    step: &str,
) -> Result<BackupKey, ApiError> {
    if passphrase.is_empty() {
        return Err(ApiError::bad_request(
            "backup.restore_passphrase_required",
            "Enter the passphrase the backup was made with",
        ));
    }
    match inspect::key_for(archive, passphrase).await {
        Ok(key) => Ok(key),
        Err(error) => {
            if error.code == "backup.restore_passphrase_wrong" {
                crate::audit::record(
                    state,
                    AuditEvent::failure(AuditAction::BackupRestored)
                        .by(audit)
                        .target("backup", "full_backup")
                        .detail("step", step)
                        .detail("reason", "passphrase_wrong"),
                )
                .await;
            }
            Err(api_error(error))
        }
    }
}

/// Whether the test restore only checks, or the restore stages.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Mode {
    Test,
    Restore,
}

/// Stages a checked copy for the next start and audits it; answers with what was staged and
/// the report of its checks.
pub(crate) async fn stage(
    state: &AppState,
    audit: &AuditContext,
    checked: Checked,
) -> Result<(PendingRestore, RestoreReportResponse), ApiError> {
    let layout = layout(state);
    let pending = PendingRestore {
        phase: Phase::Staged,
        staged_at: Utc::now(),
        archive_name: checked.archive_name.clone(),
        backup_created_at: checked.manifest.created_at,
        app_version: checked.manifest.app_version.clone(),
        minted_secrets: checked.minted.clone(),
    };
    let work = checked.work.clone();
    let staged = {
        let layout = layout.clone();
        let pending = pending.clone();
        tokio::task::spawn_blocking(move || cutover::stage(&layout, &work, &pending))
            .await
            .map_err(anyhow::Error::from)?
    };
    if let Err(error) = staged {
        restore_checks::forget_minted(state, &checked.minted).await;
        if let Err(cleanup) = std::fs::remove_dir_all(&checked.work) {
            tracing::warn!(%cleanup, "a refused restore's work folder could not be removed");
        }
        return Err(ApiError::conflict(
            "backup.restore_pending_exists",
            format!("{error:#}"),
        ));
    }
    crate::audit::record(
        state,
        AuditEvent::success(AuditAction::BackupRestored)
            .by(audit)
            .target("backup", "full_backup")
            .detail("step", "staged")
            .detail("archive", &checked.archive_name)
            .detail("moved_paths", checked.report.moved_paths)
            .detail("credentials", checked.report.restored_credentials),
    )
    .await;
    Ok((pending, checked.report))
}

/// Whether a stored path is absolute on this machine; for the preview's path list.
pub(crate) fn native(path: &str) -> bool {
    paths::is_native_absolute(path)
}

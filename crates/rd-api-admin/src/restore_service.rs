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

/// The checks after the credentials, each on the migrated copy; the updates they collect are
/// written into the copy before the reference checks read it.
async fn finish_checks(
    state: &AppState,
    copy: &Path,
    torrent_members: &[String],
    updates: &mut Vec<CopyUpdate>,
    findings: &mut Findings,
) -> Result<(), ApiError> {
    restore_checks::torrent_sources(state, copy, torrent_members, updates, findings).await?;
    restore_copy::apply_updates(copy, updates)
        .await
        .map_err(|error| {
            ApiError::unprocessable("backup.restore_apply_failed", format!("{error:#}"))
        })?;
    restore_checks::references(copy, findings).await?;
    restore_checks::partial_transfers(copy, findings).await?;
    Ok(())
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

/// Removes a work folder when the step that made it did not hand it on.
struct WorkFolder(Option<PathBuf>);

impl WorkFolder {
    fn keep(mut self) -> PathBuf {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for WorkFolder {
    fn drop(&mut self) {
        if let Some(folder) = self.0.take()
            && let Err(error) = std::fs::remove_dir_all(&folder)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "a restore work folder could not be removed");
        }
    }
}

/// What a checked copy is, for the restore to stage.
pub(crate) struct Checked {
    pub(crate) report: RestoreReportResponse,
    pub(crate) work: PathBuf,
    pub(crate) minted: Vec<String>,
    pub(crate) manifest: rd_backup::Manifest,
    pub(crate) archive_name: String,
}

/// Unpacks, migrates, remaps and checks a copy. In `Mode::Restore` the credentials go into the
/// secret store and the copy stays for staging; in `Mode::Test` the copy is removed on return.
pub(crate) async fn check(
    state: &AppState,
    audit: &AuditContext,
    request: &RestoreRequest,
    mode: Mode,
) -> Result<Checked, ApiError> {
    let archive = resolve_source(state, &request.source).await?;
    let step = if mode == Mode::Test {
        "test"
    } else {
        "restore"
    };
    let key = open(state, audit, &archive, &request.passphrase, step).await?;
    let layout = layout(state);
    tokio::fs::create_dir_all(layout.work())
        .await
        .map_err(anyhow::Error::from)?;
    let work = WorkFolder(Some(layout.work().join(uuid::Uuid::now_v7().to_string())));
    let folder = work.0.clone().unwrap_or_default();
    let unpack_key = BackupKey::from_stored(key.key_bytes(), &key.salt())?;
    let manifest = inspect::unpack(&archive, unpack_key, &folder)
        .await
        .map_err(api_error)?;
    let copy = folder.join(DATABASE_PART);

    let schema = restore_copy::copy_schema(&copy)
        .await
        .map_err(|error| ApiError::bad_request("backup.restore_damaged", format!("{error:#}")))?;
    if schema.is_newer() {
        return Err(ApiError::unprocessable(
            "backup.restore_schema_newer",
            "A newer version of rDownloader made this backup; update before restoring it",
        )
        .with_param("version", &manifest.app_version));
    }
    restore_copy::migrate_copy(&copy).await.map_err(|error| {
        ApiError::unprocessable("backup.restore_migration_failed", format!("{error:#}"))
    })?;
    let counts = restore_copy::copy_counts(&copy).await?;

    let mappings: Vec<RequestedMapping> = request
        .mappings
        .iter()
        .map(|mapping| RequestedMapping {
            storage_root_id: mapping.storage_root_id.clone(),
            path: mapping.path.clone(),
        })
        .collect();
    let path_plan = plan::plan_paths(&copy, &mappings)
        .await
        .map_err(api_error)?;
    let session =
        plan::rewrite_session(&folder.join("torrent-session"), &path_plan.mappings, false)
            .await
            .map_err(api_error)?;

    let bundle: SettingsBundle = serde_json::from_slice(
        &tokio::fs::read(folder.join(SETTINGS_PART))
            .await
            .map_err(anyhow::Error::from)?,
    )
    .map_err(|error| {
        ApiError::unprocessable("backup.restore_settings_unreadable", error.to_string())
    })?;
    crate::settings_backup::validate_header(&bundle)?;
    // Where the copy's roots land here passes the check a root created by hand does, against
    // the scripts and vendor directory the backup brings back as well.
    let protected =
        crate::protected_roots::protected_directories(state, Some(&bundle.settings)).await;
    for path in path_plan.roots.iter().filter_map(lands_on) {
        crate::protected_roots::refuse_protected(&path, &protected)?;
    }

    let mut findings = Findings::default();
    let mut updates: Vec<CopyUpdate> = path_plan.updates.clone();
    let mut minted = Vec::new();
    let outcome = restore_checks::credentials(
        state,
        restore_checks::Credentials {
            copy: &copy,
            bundle: &bundle,
            passphrase: &request.passphrase,
            key: &key,
            mint: mode == Mode::Restore,
        },
        &mut updates,
        &mut minted,
        &mut findings,
    )
    .await;
    let torrent_members: Vec<String> = manifest
        .parts
        .iter()
        .filter(|part| part.kind == PartKind::TorrentFile)
        .filter_map(|part| {
            part.name
                .strip_prefix(&format!("{TORRENT_FILES_PREFIX}/"))
                .map(str::to_owned)
        })
        .collect();
    let result = match outcome {
        Ok(()) => finish_checks(state, &copy, &torrent_members, &mut updates, &mut findings).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        restore_checks::forget_minted(state, &minted).await;
        return Err(error);
    }

    findings.error_paths("backup.restore_path_escape", &path_plan.escapes);
    findings.error_paths("backup.restore_path_escape", &session.escapes);
    findings.warn_paths("backup.restore_path_foreign", &path_plan.foreign);
    findings.warn_paths("backup.restore_path_foreign", &session.foreign);
    if !session.missing_files.is_empty() {
        findings.warn(
            "backup.restore_torrent_file_missing",
            session.missing_files.len(),
            session.missing_files.clone(),
        );
    }
    let roots = roots_report(&path_plan, &mut findings).await;

    let report = RestoreReportResponse {
        ok: !findings.has_errors(),
        schema: RestoreSchemaResponse {
            applied: schema.applied,
            known: schema.known,
            migrated: schema.pending,
        },
        counts: RestoreCountsResponse {
            packages: counts.packages,
            downloads: counts.downloads,
            unfinished: counts.unfinished,
            torrents: counts.torrents,
            storage_roots: counts.storage_roots,
            categories: counts.categories,
            accounts: counts.accounts,
            hotfolders: counts.hotfolders,
        },
        roots,
        moved_paths: path_plan.moved + session.moved,
        restored_credentials: findings.restored_credentials,
        problems: findings.into_problems(),
    };
    let archive_name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if mode == Mode::Test {
        restore_checks::forget_minted(state, &minted).await;
        return Ok(Checked {
            report,
            work: PathBuf::new(),
            minted: Vec::new(),
            manifest,
            archive_name,
        });
    }
    Ok(Checked {
        report,
        work: work.keep(),
        minted,
        manifest,
        archive_name,
    })
}

/// Where a root of the copy lands on this machine: its mapping, or its own path when that is
/// one here; a foreign path nobody mapped lands nowhere.
fn lands_on(root: &plan::CopyRoot) -> Option<PathBuf> {
    root.mapped_to
        .clone()
        .or_else(|| root.native.then(|| PathBuf::from(&root.path)))
}

async fn roots_report(
    path_plan: &plan::PathPlan,
    findings: &mut Findings,
) -> Vec<RestoreRootPlanResponse> {
    let mut missing = PathFindings::default();
    let mut roots = Vec::with_capacity(path_plan.roots.len());
    for root in &path_plan.roots {
        let lands_on = lands_on(root);
        let exists_here = match &lands_on {
            Some(path) => tokio::fs::metadata(path)
                .await
                .is_ok_and(|metadata| metadata.is_dir()),
            None => false,
        };
        if let Some(path) = &lands_on
            && !exists_here
        {
            missing.count += 1;
            missing.examples.push(plan::PathExample {
                location: "storage_roots.path".to_owned(),
                value: path.to_string_lossy().into_owned(),
            });
        }
        roots.push(RestoreRootPlanResponse {
            id: root.id.clone(),
            path: root.path.clone(),
            native: root.native,
            mapped_to: root
                .mapped_to
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            exists_here,
        });
    }
    findings.warn_paths("backup.restore_root_missing", &missing);
    roots
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

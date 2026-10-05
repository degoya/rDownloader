//! The checks of an unpacked copy, shared by the test restore and the restore.

use super::*;

/// The checks after the credentials, each on the migrated copy; the updates they collect are
/// written into the copy before the reference checks read it.
pub(super) async fn finish_checks(
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

/// Removes a work folder when the step that made it did not hand it on.
pub(super) struct WorkFolder(Option<PathBuf>);

impl WorkFolder {
    pub(super) fn keep(mut self) -> PathBuf {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for WorkFolder {
    // Blocking on purpose: a `Drop` cannot await, and it runs only when a check ends early
    // (RD-1110-06).
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

    let (schema, counts) = upgraded_copy(&copy, &manifest).await?;

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

    let bundle = checked_bundle(state, &folder, &path_plan.roots).await?;

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
    let torrent_members = torrent_members(&manifest);
    let result = match outcome {
        Ok(()) => finish_checks(state, &copy, &torrent_members, &mut updates, &mut findings).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        restore_checks::forget_minted(state, &minted).await;
        return Err(error);
    }

    path_findings(&mut findings, &path_plan, &session);
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
pub(super) fn lands_on(root: &plan::CopyRoot) -> Option<PathBuf> {
    root.mapped_to
        .clone()
        .or_else(|| root.native.then(|| PathBuf::from(&root.path)))
}

pub(super) async fn roots_report(
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

/// Reads the copy's schema, refuses one a newer version made, migrates it and counts it.
async fn upgraded_copy(
    copy: &Path,
    manifest: &rd_backup::Manifest,
) -> Result<(restore_copy::CopySchema, restore_copy::CopyCounts), ApiError> {
    let schema = restore_copy::copy_schema(copy)
        .await
        .map_err(|error| ApiError::bad_request("backup.restore_damaged", format!("{error:#}")))?;
    if schema.is_newer() {
        return Err(ApiError::unprocessable(
            "backup.restore_schema_newer",
            "A newer version of rDownloader made this backup; update before restoring it",
        )
        .with_param("version", &manifest.app_version));
    }
    restore_copy::migrate_copy(copy).await.map_err(|error| {
        ApiError::unprocessable("backup.restore_migration_failed", format!("{error:#}"))
    })?;
    let counts = restore_copy::copy_counts(copy).await?;
    Ok((schema, counts))
}

/// The settings bundle of the copy, once its header and the roots it brings back passed.
async fn checked_bundle(
    state: &AppState,
    folder: &Path,
    roots: &[plan::CopyRoot],
) -> Result<SettingsBundle, ApiError> {
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
    for path in roots.iter().filter_map(lands_on) {
        crate::protected_roots::refuse_protected(&path, &protected)?;
    }
    Ok(bundle)
}

/// The torrent files the backup carries, by their names below the torrent folder.
fn torrent_members(manifest: &rd_backup::Manifest) -> Vec<String> {
    manifest
        .parts
        .iter()
        .filter(|part| part.kind == PartKind::TorrentFile)
        .filter_map(|part| {
            part.name
                .strip_prefix(&format!("{TORRENT_FILES_PREFIX}/"))
                .map(str::to_owned)
        })
        .collect()
}

/// What the path plan and the torrent session found: escapes, foreign paths, missing files.
fn path_findings(
    findings: &mut Findings,
    path_plan: &plan::PathPlan,
    session: &plan::SessionReport,
) {
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
}

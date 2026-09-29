//! One backup run, from the snapshot to the archive at its destination (RD-160-01).
//!
//! The order is what makes it one consistent point:
//!
//! 1. the database copy, taken in the writer's order (`rd_db::snapshot`);
//! 2. the torrent session and the stored `.torrent` files, right after it. The engine writes
//!    its session file by rename, so every copied file is whole; and the database is the
//!    authority on a restore — the start drops every session torrent no queue row claims
//!    (`rd_torrent`'s `drop_orphans`), so a torrent added in between cannot survive alone;
//! 3. the plugin trust and the unfinished transfers, read out of the *copy*, not the live
//!    database, so they describe the same instant as the copy;
//! 4. the manifest over all of it, and the sealed archive ([`seal_backup`]);
//! 5. the archive handed to its destinations — every one of them gets a copy (RD-160-02,
//!    `crate::deliver`); [`create_backup`] is the one-destination form of both steps.
//!
//! Everything is staged below `<data directory>/backup-staging/<run>`; a run that fails leaves
//! it for [`sweep_staging`], which the service runs after every run and at every start. The
//! queue is never touched: the only write this makes to the live database is the snapshot
//! command, and a failure anywhere is this run's failure only.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::archive::{self, digest_file};
use crate::crypto::BackupKey;
use crate::destination::BackupDestination;
use crate::manifest::{
    DATABASE_PART, Manifest, ManifestPart, PARTIAL_TRANSFERS_PART, PLUGIN_TRUST_PART, PartKind,
    SETTINGS_PART, TORRENT_FILES_PREFIX, TORRENT_SESSION_PREFIX, is_safe_member_name,
};

/// The folder below the data directory runs stage in.
pub const STAGING_DIR: &str = "backup-staging";

/// What the caller hands a run besides the database.
pub struct BackupSources {
    /// The settings bundle as JSON, its credentials already sealed under the backup key.
    pub settings_bundle: Vec<u8>,
    /// The torrent engine's session folder.
    pub torrent_session: Option<PathBuf>,
    /// The folder uploaded `.torrent` files are stored in.
    pub torrent_files: Option<PathBuf>,
    /// The version of rDownloader writing the archive.
    pub app_version: String,
    /// This installation's id, part of every archive name so retention can tell its own
    /// archives from anybody else's (RD-160-02).
    pub instance_id: String,
}

/// A sealed archive in the staging folder, not yet at any destination.
#[derive(Clone, Debug)]
pub struct SealedBackup {
    /// The sealed file, below the staging root; gone with the next sweep.
    pub path: PathBuf,
    /// The name it gets at every destination.
    pub archive_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub manifest: Manifest,
}

/// A run that reached its destination.
#[derive(Clone, Debug)]
pub struct CreatedBackup {
    pub archive_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub location: String,
    pub manifest: Manifest,
}

/// Why a run failed: a stable code for the history and the interface, and the detail.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct BackupError {
    pub code: &'static str,
    pub detail: String,
}

impl BackupError {
    fn at(code: &'static str) -> impl FnOnce(anyhow::Error) -> Self {
        move |error| Self {
            code,
            detail: format!("{error:#}"),
        }
    }
}

/// Where runs stage, below the data directory.
#[must_use]
pub fn staging_root(data_directory: &Path) -> PathBuf {
    data_directory.join(STAGING_DIR)
}

/// What every archive name starts with.
pub const ARCHIVE_PREFIX: &str = "rdownloader-backup-";

/// The archive's file name for a run of installation `instance` started at `at`: sortable
/// within one installation, the same on every system, and telling installations apart
/// (`crate::retention::is_own_archive`).
#[must_use]
pub fn archive_name(instance: &str, at: DateTime<Utc>) -> String {
    format!(
        "{ARCHIVE_PREFIX}{instance}-{}.{}",
        at.format("%Y%m%dT%H%M%SZ"),
        crate::ARCHIVE_EXTENSION
    )
}

/// Removes everything a run left in the staging folder. Nothing in it is ever the only copy of
/// anything: it holds copies of what is in the live database and folders, and archives that
/// did not reach their destination.
///
/// # Errors
///
/// When the folder exists and cannot be removed.
pub async fn sweep_staging(root: &Path) -> anyhow::Result<()> {
    match tokio::fs::remove_dir_all(root).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(anyhow::Error::new(error)
                .context(format!("remove backup staging {}", root.display())))
        }
    }
}

/// Runs one backup to one destination: [`seal_backup`], then the destination's `store`.
///
/// # Errors
///
/// With the stage that failed as the code: `backup.snapshot_failed`, `backup.collect_failed`,
/// `backup.archive_failed`, or the destination's own code.
pub async fn create_backup(
    database: &rd_db::Database,
    sources: BackupSources,
    key: &BackupKey,
    destination: &dyn BackupDestination,
    staging_root: &Path,
    run_id: &str,
    started_at: DateTime<Utc>,
) -> Result<CreatedBackup, BackupError> {
    let sealed = seal_backup(database, sources, key, staging_root, run_id, started_at).await?;
    let stored = destination
        .store(&sealed.path, &sealed.archive_name)
        .await
        .map_err(|error| BackupError {
            code: error.code(),
            detail: error.to_string(),
        })?;
    Ok(CreatedBackup {
        archive_name: sealed.archive_name,
        size_bytes: sealed.size_bytes,
        sha256: sealed.sha256,
        location: stored.location,
        manifest: sealed.manifest,
    })
}

/// Snapshot, parts, manifest, sealed archive — everything up to the destinations.
///
/// # Errors
///
/// With the stage that failed as the code: `backup.snapshot_failed`, `backup.collect_failed`
/// or `backup.archive_failed`.
pub async fn seal_backup(
    database: &rd_db::Database,
    sources: BackupSources,
    key: &BackupKey,
    staging_root: &Path,
    run_id: &str,
    started_at: DateTime<Utc>,
) -> Result<SealedBackup, BackupError> {
    let staging = staging_root.join(run_id);
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|error| BackupError::at("backup.collect_failed")(error.into()))?;

    database
        .snapshot_into(&staging.join(DATABASE_PART))
        .await
        .map_err(BackupError::at("backup.snapshot_failed"))?;
    // The staging now holds an unencrypted copy of the database; a stop from here on leaves it
    // for the sweep every start runs.
    rd_core::failpoint!("backup.after_database_snapshot", || BackupError {
        code: "backup.interrupted",
        detail: "crash point".to_owned(),
    });
    let mut parts = vec![(DATABASE_PART.to_owned(), PartKind::Database)];
    parts.extend(
        collect(&staging, &sources)
            .await
            .map_err(BackupError::at("backup.collect_failed"))?,
    );

    let manifest_parts = {
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || describe(&staging, parts))
            .await
            .map_err(|error| BackupError::at("backup.collect_failed")(error.into()))?
            .map_err(BackupError::at("backup.collect_failed"))?
    };
    let manifest = Manifest::new(started_at, sources.app_version, manifest_parts);

    let name = archive_name(&sources.instance_id, started_at);
    let sealed = staging_root.join(format!("{run_id}.{}", crate::ARCHIVE_EXTENSION));
    let summary = {
        let staging = staging.clone();
        let manifest = manifest.clone();
        let sealed = sealed.clone();
        let key = BackupKey::from_stored(key.key_bytes(), &key.salt())
            .map_err(BackupError::at("backup.archive_failed"))?;
        tokio::task::spawn_blocking(move || {
            archive::write_archive(&staging, &manifest, &key, &sealed)
        })
        .await
        .map_err(|error| BackupError::at("backup.archive_failed")(error.into()))?
        .map_err(BackupError::at("backup.archive_failed"))?
    };

    rd_core::failpoint!("backup.before_archive_published", || BackupError {
        code: "backup.interrupted",
        detail: "crash point".to_owned(),
    });

    Ok(SealedBackup {
        path: sealed,
        archive_name: name,
        size_bytes: summary.size_bytes,
        sha256: summary.sha256,
        manifest,
    })
}

/// Writes the settings bundle, the plugin trust and the partial transfers, and copies the
/// torrent folders; returns each part's member name and kind.
async fn collect(
    staging: &Path,
    sources: &BackupSources,
) -> anyhow::Result<Vec<(String, PartKind)>> {
    let snapshot = staging.join(DATABASE_PART);
    let mut parts = Vec::new();

    tokio::fs::write(staging.join(SETTINGS_PART), &sources.settings_bundle).await?;
    parts.push((SETTINGS_PART.to_owned(), PartKind::Settings));

    let trust =
        rd_db::snapshot::read_tables(&snapshot, rd_db::snapshot::PLUGIN_TRUST_TABLES).await?;
    tokio::fs::write(
        staging.join(PLUGIN_TRUST_PART),
        serde_json::to_vec_pretty(&serde_json::json!({ "tables": trust }))?,
    )
    .await?;
    parts.push((PLUGIN_TRUST_PART.to_owned(), PartKind::PluginTrust));

    let partial = rd_db::snapshot::read_partial_transfers(&snapshot).await?;
    tokio::fs::write(
        staging.join(PARTIAL_TRANSFERS_PART),
        serde_json::to_vec_pretty(&serde_json::json!({ "downloads": partial }))?,
    )
    .await?;
    parts.push((
        PARTIAL_TRANSFERS_PART.to_owned(),
        PartKind::PartialTransfers,
    ));

    for (folder, prefix, kind) in [
        (
            &sources.torrent_session,
            TORRENT_SESSION_PREFIX,
            PartKind::TorrentSession,
        ),
        (
            &sources.torrent_files,
            TORRENT_FILES_PREFIX,
            PartKind::TorrentFile,
        ),
    ] {
        let Some(folder) = folder.clone() else {
            continue;
        };
        let target = staging.join(prefix);
        let copied =
            tokio::task::spawn_blocking(move || copy_folder(&folder, &target, prefix)).await??;
        parts.extend(copied.into_iter().map(|name| (name, kind)));
    }
    Ok(parts)
}

/// Copies every plain file below `from` into `to`, returning the member names under `prefix`.
///
/// Symbolic links are not followed and temporary files (`*.tmp`) are left out: the first could
/// point anywhere, the second is a write the engine had not committed yet. A folder that does
/// not exist is an empty part, not an error — a service that never ran a torrent has none.
fn copy_folder(from: &Path, to: &Path, prefix: &str) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    if !from.is_dir() {
        return Ok(names);
    }
    // `relative` is the path below `from`, `/`-separated; the member name is it under `prefix`.
    let mut pending = vec![(from.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                tracing::warn!(
                    path = %entry.path().display(),
                    "backup skips a file name that is not UTF-8"
                );
                continue;
            };
            let below = if relative.is_empty() {
                file_name.clone()
            } else {
                format!("{relative}/{file_name}")
            };
            let name = format!("{prefix}/{below}");
            if !is_safe_member_name(&name) {
                continue;
            }
            if file_type.is_dir() {
                pending.push((entry.path(), below));
            } else if file_type.is_file() && !file_name.ends_with(".tmp") {
                let target = to.join(&below);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                match std::fs::copy(entry.path(), &target) {
                    Ok(_) => names.push(name),
                    // Removed by the engine between listing and copying: it is gone from the
                    // session too, which is what the copy should say.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Sizes and digests of every staged part, in the order given.
fn describe(staging: &Path, parts: Vec<(String, PartKind)>) -> anyhow::Result<Vec<ManifestPart>> {
    parts
        .into_iter()
        .map(|(name, kind)| {
            let (size, sha256) = digest_file(&staging.join(&name))?;
            Ok(ManifestPart {
                name,
                kind,
                size,
                sha256,
            })
        })
        .collect()
}

//! The backup the updater asks for before it switches versions (RD-180-03).
//!
//! Two parts, both written to `<data directory>/pre-update/`, a folder of their own with its own
//! retention — the scheduled backup's destinations and their retention never see them:
//!
//! 1. [`copy_database`]: a consistent copy of the database, `VACUUM INTO` as a writer command
//!    like every full backup's snapshot, named like the copy a start takes before it migrates
//!    (`rd_db::pre_migration::copy_name`, app versions in place of migration numbers). It is
//!    written under a `.partial` name, synced, checked — `PRAGMA integrity_check`, the schema
//!    this build runs on and nothing pending, the core tables readable — and only then renamed
//!    to its name. A copy under its name is therefore always whole and checked.
//! 2. [`seal_archive`]: the encrypted full backup of RD-160-01, sealed in `pre-update/staging`
//!    under the configured key, opened again with that key and read to its end
//!    (`archive::verify_archive`: every chunk, every member against the manifest), and only
//!    then renamed into the folder. The staging, which holds an unencrypted database copy while
//!    the archive is sealed, is removed afterwards.
//!
//! A stop at any point leaves at most a `.partial` file or the staging folder, never a file
//! under a final name; [`sweep`] removes both, and the service runs it at every start and before
//! and after every preparation. The live database is only read. The newest [`KEPT`] copies and
//! [`KEPT`] archives stay.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::archive::{self, digest_file};
use crate::create::{BackupError, BackupSources, archive_name, seal_backup, sweep_staging};
use crate::crypto::BackupKey;

/// One preparation or sweep at a time in this process. The start's sweep of a stopped
/// preparation runs in the background, and without this it removed the `.partial` copy of a
/// preparation that had just begun (`sync …: No such file or directory`).
static FOLDER_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The folder below the data directory everything here is written to.
pub const DIRECTORY: &str = "pre-update";

/// How many database copies, and separately how many archives, stay.
pub const KEPT: usize = 3;

/// The database copy could not be written.
pub const COPY_FAILED: &str = "update.copy_failed";
/// The database copy was written but did not pass its check.
pub const COPY_DAMAGED: &str = "update.copy_damaged";
/// The archive was sealed but did not open again under its key, or its digest changed.
pub const ARCHIVE_DAMAGED: &str = "update.archive_damaged";
/// The archive could not be moved into the folder.
pub const ARCHIVE_FAILED: &str = "update.archive_failed";

const STAGING: &str = "staging";
const PARTIAL: &str = ".partial";
/// The run id the archive is sealed under inside the staging folder.
const RUN: &str = "pre-update";

/// The versions a preparation is between, and when it was asked for.
#[derive(Clone, Copy, Debug)]
pub struct UpdatePlan<'a> {
    pub data_directory: &'a Path,
    /// The version running now.
    pub from_version: &'a str,
    /// The version the updater switches to.
    pub target_version: &'a str,
    pub at: DateTime<Utc>,
}

/// A database copy that passed its check.
#[derive(Clone, Debug)]
pub struct VerifiedCopy {
    pub path: PathBuf,
    pub size_bytes: u64,
    /// The highest migration the copy has applied: the schema the running version needs.
    pub schema_version: i64,
    pub counts: rd_db::restore_copy::CopyCounts,
}

/// An archive that was opened again with its key and read to its end.
#[derive(Clone, Debug)]
pub struct VerifiedArchive {
    pub path: PathBuf,
    pub archive_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// The fingerprint of the key it is sealed under.
    pub key_fingerprint: String,
}

/// `<data directory>/pre-update`.
#[must_use]
pub fn directory(data_directory: &Path) -> PathBuf {
    data_directory.join(DIRECTORY)
}

fn failure(code: &'static str, detail: impl Into<String>) -> BackupError {
    BackupError {
        code,
        detail: detail.into(),
    }
}

/// Whether a version may become part of a file name: what a version string needs and nothing
/// that could leave the folder.
#[must_use]
pub fn is_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && !version.starts_with('.')
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character))
}

/// Writes, checks and publishes the database copy, then rotates the copies.
///
/// # Errors
///
/// [`COPY_FAILED`] when the copy cannot be written or published, [`COPY_DAMAGED`] when it does
/// not pass its check. Either leaves at most the `.partial` file for [`sweep`].
pub async fn copy_database(
    database: &rd_db::Database,
    plan: UpdatePlan<'_>,
) -> Result<VerifiedCopy, BackupError> {
    let _folder = FOLDER_LOCK.lock().await;
    if !is_version(plan.from_version) || !is_version(plan.target_version) {
        return Err(failure(
            COPY_FAILED,
            "a version is not usable in a file name",
        ));
    }
    let folder = directory(plan.data_directory);
    private_folder(&folder)
        .await
        .map_err(|error| failure(COPY_FAILED, format!("create {}: {error}", folder.display())))?;
    let name = rd_db::pre_migration::copy_name(plan.from_version, plan.target_version, plan.at);
    let partial = folder.join(format!("{name}{PARTIAL}"));
    database
        .snapshot_into(&partial)
        .await
        .map_err(|error| failure(COPY_FAILED, format!("{error:#}")))?;
    sync(&partial)
        .await
        .map_err(|error| failure(COPY_FAILED, format!("sync {}: {error}", partial.display())))?;
    // The copy is on disk under its `.partial` name and has not been checked: a stop from here
    // on leaves it for the sweep, never under the name a copy is trusted by.
    rd_core::failpoint!("pre_update.before_copy_published", || failure(
        "backup.interrupted",
        "crash point"
    ));
    rd_db::snapshot::check_integrity(&partial)
        .await
        .map_err(|error| failure(COPY_DAMAGED, format!("{error:#}")))?;
    let schema = rd_db::restore_copy::copy_schema(&partial)
        .await
        .map_err(|error| failure(COPY_DAMAGED, format!("{error:#}")))?;
    let schema_version = match schema.applied {
        Some(applied) if schema.pending == 0 && !schema.is_newer() => applied,
        _ => {
            return Err(failure(
                COPY_DAMAGED,
                format!(
                    "the copy is not at the schema this version runs on (applied {:?}, {} \
                     pending, unknown {:?})",
                    schema.applied, schema.pending, schema.unknown
                ),
            ));
        }
    };
    let counts = rd_db::restore_copy::copy_counts(&partial)
        .await
        .map_err(|error| failure(COPY_DAMAGED, format!("{error:#}")))?;
    let path = folder.join(&name);
    tokio::fs::rename(&partial, &path)
        .await
        .map_err(|error| failure(COPY_FAILED, format!("publish {}: {error}", path.display())))?;
    let size_bytes = tokio::fs::metadata(&path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    rd_db::pre_migration::rotate(&folder, KEPT).await;
    Ok(VerifiedCopy {
        path,
        size_bytes,
        schema_version,
        counts,
    })
}

/// Seals the full backup under `key`, opens it again and reads it to its end, publishes it in
/// the folder and rotates the archives there.
///
/// # Errors
///
/// The sealing stage's own code (`backup.snapshot_failed`, `backup.collect_failed`,
/// `backup.archive_failed`), [`ARCHIVE_DAMAGED`] when the check fails, [`ARCHIVE_FAILED`] when
/// it cannot be moved into the folder. Either leaves at most the staging folder for [`sweep`].
pub async fn seal_archive(
    database: &rd_db::Database,
    sources: BackupSources,
    key: &BackupKey,
    plan: UpdatePlan<'_>,
) -> Result<VerifiedArchive, BackupError> {
    let _folder = FOLDER_LOCK.lock().await;
    let folder = directory(plan.data_directory);
    let staging = folder.join(STAGING);
    // Anything an earlier stop left there is no archive anybody has.
    sweep_staging(&staging)
        .await
        .map_err(|error| failure(ARCHIVE_FAILED, format!("{error:#}")))?;
    // The staging holds an unencrypted copy of the database until the archive is sealed.
    for private in [&folder, &staging] {
        private_folder(private).await.map_err(|error| {
            failure(
                ARCHIVE_FAILED,
                format!("create {}: {error}", private.display()),
            )
        })?;
    }
    let sealed = seal_backup(database, sources, key, &staging, RUN, plan.at).await?;
    let checked = {
        let path = sealed.path.clone();
        let key = BackupKey::from_stored(key.key_bytes(), &key.salt())
            .map_err(|error| failure(ARCHIVE_DAMAGED, format!("{error:#}")))?;
        tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, String)> {
            let digest = digest_file(&path)?;
            archive::verify_archive(&path, &key)?;
            Ok(digest)
        })
        .await
        .map_err(|error| failure(ARCHIVE_DAMAGED, error.to_string()))?
        .map_err(|error| failure(ARCHIVE_DAMAGED, format!("{error:#}")))?
    };
    if checked != (sealed.size_bytes, sealed.sha256.clone()) {
        return Err(failure(
            ARCHIVE_DAMAGED,
            format!(
                "{} holds {} bytes with SHA-256 {}; {} bytes with {} were sealed",
                sealed.path.display(),
                checked.0,
                checked.1,
                sealed.size_bytes,
                sealed.sha256
            ),
        ));
    }
    // Checked and complete in staging, not yet in the folder: a stop here leaves it for the
    // sweep, and the folder never shows an archive that was not checked.
    rd_core::failpoint!("pre_update.before_archive_published", || failure(
        "backup.interrupted",
        "crash point"
    ));
    let name = archive_name(&format!("{RUN}-{}", plan.target_version), plan.at);
    let path = folder.join(&name);
    tokio::fs::rename(&sealed.path, &path)
        .await
        .map_err(|error| {
            failure(
                ARCHIVE_FAILED,
                format!("publish {}: {error}", path.display()),
            )
        })?;
    if let Err(error) = sweep_staging(&staging).await {
        tracing::warn!(%error, "the pre-update staging folder could not be emptied");
    }
    rotate_archives(&folder, KEPT).await;
    Ok(VerifiedArchive {
        path,
        archive_name: name,
        size_bytes: sealed.size_bytes,
        sha256: sealed.sha256,
        key_fingerprint: key.fingerprint(),
    })
}

/// Removes what a stopped preparation left: `.partial` copies and the staging folder. Files
/// under a final name are never touched.
///
/// # Errors
///
/// When the folder exists and cannot be read, or a leftover cannot be removed.
pub async fn sweep(data_directory: &Path) -> anyhow::Result<()> {
    let _folder = FOLDER_LOCK.lock().await;
    let folder = directory(data_directory);
    sweep_staging(&folder.join(STAGING)).await?;
    let mut entries = match tokio::fs::read_dir(&folder).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name().to_string_lossy().ends_with(PARTIAL) {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

/// Creates `folder` for the service account alone and makes an existing one so (security review
/// 2026-09-30, finding 8): it holds plain copies of the database, readable by every account on
/// the machine under the default umask.
async fn private_folder(folder: &Path) -> std::io::Result<()> {
    let folder = folder.to_path_buf();
    tokio::task::spawn_blocking(move || {
        rd_files::create_private_dir_all(&folder)?;
        rd_files::restrict_to_owner(&folder)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Opened for writing: Windows flushes a file only through a handle with write access.
async fn sync(path: &Path) -> std::io::Result<()> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await?
        .sync_all()
        .await
}

/// Removes every archive in the folder but the newest `keep`; the names sort by their time.
async fn rotate_archives(folder: &Path, keep: usize) {
    let Ok(mut entries) = tokio::fs::read_dir(folder).await else {
        return;
    };
    let prefix = format!("{}{RUN}-", crate::ARCHIVE_PREFIX);
    let mut archives = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(&prefix) && crate::is_archive_name(&name) {
            // The timestamp is the last segment before the extension, fixed width.
            let stamp = name
                .trim_end_matches(crate::ARCHIVE_EXTENSION)
                .trim_end_matches('.')
                .rsplit('-')
                .next()
                .unwrap_or_default()
                .to_owned();
            archives.push((stamp, entry.path()));
        }
    }
    archives.sort();
    let surplus = archives.len().saturating_sub(keep);
    for (_, path) in archives.into_iter().take(surplus) {
        if let Err(error) = tokio::fs::remove_file(&path).await {
            tracing::warn!(%error, archive = %path.display(), "an old pre-update archive could not be removed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_version;

    #[test]
    fn only_version_strings_become_part_of_a_file_name() {
        for good in ["1.8.0", "1.8.0-beta.2", "1.8.0+build.7"] {
            assert!(is_version(good), "{good}");
        }
        for bad in [
            "",
            "../1.8.0",
            "1.8/0",
            "1.8\\0",
            ".hidden",
            "1.8.0 beta",
            &"9".repeat(65),
        ] {
            assert!(!is_version(bad), "{bad}");
        }
    }
}

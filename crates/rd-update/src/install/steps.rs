//! The steps every install kind shares (RD-180-02): the artifact checked once more before it is
//! used, the database copy put back after a failed start, and the leftovers of a finished update.

use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use super::{
    INSTALLER_DIR, InstallError, Journal, Phase, Plan, exists, remove_any, rename, sync_directory,
    trash, update_dir,
};
use crate::InstallKind;

/// Hashes the downloaded artifact again: the file lay in the data directory between the service's
/// check and this use, and only what the signed manifest describes is ever installed.
///
/// # Errors
///
/// `update.digest_mismatch` when size or SHA-256 differ, `update.download_failed` when it is gone.
pub fn verify_artifact(plan: &Plan) -> Result<(), InstallError> {
    open_artifact(plan).map(drop)
}

/// [`open_checked`] for the plan's artifact: the handle the switch unpacks from, or holds while
/// Windows Installer reads the package.
///
/// # Errors
///
/// As [`verify_artifact`].
pub fn open_artifact(plan: &Plan) -> Result<fs::File, InstallError> {
    open_checked(&plan.artifact, &plan.sha256, Some(plan.size))
}

/// Opens `path`, checks its SHA-256 (and its size, when given) and returns the handle at the
/// file's start (security review 2026-09-30, finding 6): what is read through it is what was
/// hashed, where hashing one opening and using the path again let a file swapped in between be
/// installed. On Windows the handle shares reading only, so while it is open nobody writes,
/// renames or removes the file — `msiexec` reads the package it was checked as.
///
/// # Errors
///
/// `update.digest_mismatch` when size or SHA-256 differ, `update.download_failed` when the file
/// cannot be read.
pub fn open_checked(
    path: &Path,
    sha256: &str,
    size: Option<u64>,
) -> Result<fs::File, InstallError> {
    let gone = |error: std::io::Error| {
        InstallError::new(
            "update.download_failed",
            format!("{}: {error}", path.display()),
        )
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    let mut file = options.open(path).map_err(gone)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    let mut read_bytes: u64 = 0;
    loop {
        let read = file.read(&mut buffer).map_err(gone)?;
        if read == 0 {
            break;
        }
        read_bytes = read_bytes.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    if size.is_some_and(|size| size != read_bytes) || hex::encode(hasher.finalize()) != sha256 {
        return Err(InstallError::new(
            "update.digest_mismatch",
            format!("{} is not the file that was checked", path.display()),
        ));
    }
    file.seek(SeekFrom::Start(0)).map_err(gone)?;
    Ok(file)
}

/// Puts the database copy the backup before the update wrote (RD-180-03) back in place: the new
/// version was started and may have migrated the database to a schema the old one cannot open.
/// The live files go to `<data>/update/replaced-database/`, the copy is written beside the
/// database and renamed over it. Returns whether a copy was put back.
///
/// Run again after a stop, it puts the copy back again, which is the same result.
///
/// # Errors
///
/// When the copy is gone or a step fails.
pub fn restore_database(plan: &Plan) -> Result<bool> {
    let Some(copy) = plan.database_copy.as_ref() else {
        return Ok(false);
    };
    ensure!(
        copy.is_file(),
        "the database copy {} is gone",
        copy.display()
    );
    let name = plan
        .database
        .file_name()
        .context("the database has no file name")?
        .to_string_lossy()
        .into_owned();
    let aside = update_dir(&plan.data_dir).join("replaced-database");
    remove_any(&aside)?;
    fs::create_dir_all(&aside).with_context(|| format!("create {}", aside.display()))?;
    for suffix in ["", "-wal", "-shm"] {
        let live = plan.database.with_file_name(format!("{name}{suffix}"));
        if exists(&live) {
            rename(&live, &aside.join(format!("{name}{suffix}")))?;
        }
    }
    let restoring = plan.database.with_file_name(format!("{name}.restoring"));
    fs::copy(copy, &restoring)
        .with_context(|| format!("copy {} to {}", copy.display(), restoring.display()))?;
    fs::OpenOptions::new()
        .write(true)
        .open(&restoring)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("sync {}", restoring.display()))?;
    rename(&restoring, &plan.database)?;
    if let Some(parent) = plan.database.parent() {
        sync_directory(parent);
    }
    Ok(true)
}

/// Keeps the installer a verified MSI update installed, for the rollback of the next one: the
/// only way back from a Windows Installer upgrade is the previous package. Older ones go. Its
/// SHA-256 is written beside it ([`INSTALLER_DIGEST_SUFFIX`]), so the next plan carries it and a
/// rollback installs only the package that was kept.
///
/// # Errors
///
/// When the installer cannot be moved.
pub fn keep_installer(journal: &Journal) -> Result<()> {
    if journal.plan.kind != InstallKind::Msi {
        return Ok(());
    }
    let directory = update_dir(&journal.plan.data_dir).join(INSTALLER_DIR);
    remove_any(&directory)?;
    fs::create_dir_all(&directory).with_context(|| format!("create {}", directory.display()))?;
    let name = journal
        .plan
        .artifact
        .file_name()
        .context("the installer has no file name")?;
    let kept = directory.join(format!(
        "{}-{}",
        journal.plan.target_version,
        name.to_string_lossy()
    ));
    rename(&journal.plan.artifact, &kept)?;
    let digest = digest_path(&kept);
    fs::write(&digest, &journal.plan.sha256).with_context(|| format!("write {}", digest.display()))
}

/// Beside a kept installer: its SHA-256 as the update that installed it checked it.
pub const INSTALLER_DIGEST_SUFFIX: &str = ".sha256";

fn digest_path(installer: &Path) -> PathBuf {
    let mut name = installer.as_os_str().to_owned();
    name.push(INSTALLER_DIGEST_SUFFIX);
    PathBuf::from(name)
}

/// The installer [`keep_installer`] kept for `version`, if it is there.
#[must_use]
pub fn kept_installer(data: &Path, version: &str) -> Option<PathBuf> {
    let prefix = format!("{version}-");
    fs::read_dir(update_dir(data).join(INSTALLER_DIR))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .is_some_and(|name| {
                    name.starts_with(&prefix) && name.to_ascii_lowercase().ends_with(".msi")
                })
        })
}

/// The SHA-256 [`keep_installer`] recorded for `installer`; `None` without a well-formed one.
#[must_use]
pub fn kept_installer_sha256(installer: &Path) -> Option<String> {
    let digest = fs::read_to_string(digest_path(installer)).ok()?;
    let digest = digest.trim();
    is_sha256(digest).then(|| digest.to_owned())
}

/// 64 lowercase hexadecimal digits.
#[must_use]
pub fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The installer of the version before the update, checked against the digest the plan carries
/// and held open as [`open_checked`] holds it; `None` when no installer was kept or the plan
/// carries no digest for it — then there is no way back through Windows Installer.
///
/// # Errors
///
/// `update.digest_mismatch` when the kept installer changed since it was kept.
pub fn previous_installer(plan: &Plan) -> Result<Option<(PathBuf, fs::File)>, InstallError> {
    let (Some(path), Some(sha256)) = (&plan.previous_installer, &plan.previous_installer_sha256)
    else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    open_checked(path, sha256, None).map(|file| Some((path.clone(), file)))
}

/// Removes what a finished update leaves: the staging, `.previous/`, the files of a rolled-back
/// version, the downloads, the replaced database and the updater's copy. Each on its own; those
/// beside the program go through the trash ([`trash::discard`]), so what is still in use (on
/// Windows a capture agent that runs from `.previous/`) is removed by a later start.
pub fn clean_up(journal: &Journal) {
    let install = &journal.plan.install_dir;
    let mut beside_program = vec![journal.staged_dir(), journal.failed_dir()];
    // After a proven update `.previous/` holds the old version, which this start of the new one
    // no longer needs. After anything else it is either gone or what a manual recovery needs.
    if journal.phase == Phase::Verified {
        beside_program.push(journal.previous_dir());
    }
    for leftover in beside_program {
        if let Err(error) = trash::discard(&leftover, install) {
            tracing::warn!(
                error = format!("{error:#}"),
                "an update leftover stays until a later start"
            );
        }
    }
    let update = update_dir(&journal.plan.data_dir);
    for leftover in [
        update.join(super::DOWNLOAD_DIR),
        update.join("replaced-database"),
        update.join(super::process::UPDATER_DIR),
    ] {
        if let Err(error) = remove_any(&leftover) {
            tracing::warn!(%error, "an update leftover stays until a later start");
        }
    }
}

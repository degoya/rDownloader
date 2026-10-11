//! What stays of the copies kept for taking an update back (RD-1240-34).
//!
//! Three kinds of file pile up below the data directory, each rotated where it is written: the
//! database copy a start takes before it migrates (`pre-migration/`, `rd_db::pre_migration`),
//! and the database copy and the encrypted archive the updater asks for (`pre-update/`,
//! [`crate::pre_update`]), three of each. Every one of them is a whole database, so on an
//! installation with a large one they outweigh everything else in the data directory.
//!
//! Once the last update is proven — its journal says `verified` for the version running now, or
//! no update is recorded and the start that migrated came up (`update_proven` in
//! `rd_update::install::recover`) — the older ones take nothing back any more: a rollback
//! reaches for the newest copy of each kind. [`plan`] keeps exactly that one, and after
//! [`UpdateBackupPolicy::grace_days`] that one goes too (0 keeps it for good). An update that is
//! not proven — waiting for its proof, rolled back, failed — keeps everything the rotation left.
//!
//! Files neither module named are never touched, and neither are the `.partial` copies or the
//! staging folder, which [`crate::pre_update::sweep`] owns.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::pre_update::{self, archive_stamp};

/// Which of the three kinds a file is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum BackupKind {
    /// The checked database copy before an update.
    PreUpdateCopy,
    /// The encrypted full backup before an update.
    PreUpdateArchive,
    /// The database copy a start took before it migrated.
    PreMigrationCopy,
}

/// One copy or archive.
#[derive(Clone, Debug)]
pub struct BackupFile {
    pub kind: BackupKind,
    pub path: PathBuf,
    pub size_bytes: u64,
    /// The timestamp in its name, fixed width per kind: the names of one kind sort by it.
    stamp: String,
    modified: Option<SystemTime>,
}

/// What may go.
#[derive(Clone, Copy, Debug)]
pub struct UpdateBackupPolicy {
    /// The last update is proven, or none is recorded.
    pub proven: bool,
    /// Days the newest file of a kind stays after a proven update; 0 keeps it for good.
    pub grace_days: u32,
    pub now: SystemTime,
}

/// The files of the two folders, split into what stays and what goes.
#[derive(Clone, Debug, Default)]
pub struct UpdateBackupPlan {
    pub kept: Vec<BackupFile>,
    /// In a [`plan`] what may go; after [`apply`] what went.
    pub removable: Vec<BackupFile>,
}

impl UpdateBackupPlan {
    /// How many files of these kinds stay, and their bytes.
    #[must_use]
    pub fn kept_of(&self, kinds: &[BackupKind]) -> (u64, u64) {
        totals(&self.kept, kinds)
    }

    /// How many files of these kinds may go (after [`apply`]: went), and their bytes.
    #[must_use]
    pub fn removable_of(&self, kinds: &[BackupKind]) -> (u64, u64) {
        totals(&self.removable, kinds)
    }
}

fn totals(files: &[BackupFile], kinds: &[BackupKind]) -> (u64, u64) {
    files
        .iter()
        .filter(|file| kinds.contains(&file.kind))
        .fold((0, 0), |(count, bytes), file| {
            (count + 1, bytes + file.size_bytes)
        })
}

/// What [`apply`] would remove, read from the two folders below `data_directory`.
pub async fn plan(data_directory: &Path, policy: UpdateBackupPolicy) -> UpdateBackupPlan {
    let mut plan = UpdateBackupPlan::default();
    for kind in [
        BackupKind::PreUpdateCopy,
        BackupKind::PreUpdateArchive,
        BackupKind::PreMigrationCopy,
    ] {
        let files = list(data_directory, kind).await;
        let (kept, removable) = split(files, policy);
        plan.kept.extend(kept);
        plan.removable.extend(removable);
    }
    plan
}

/// Removes what [`plan`] finds removable, under the pre-update folder's lock so no preparation
/// writes meanwhile. A file that cannot be removed stays in `kept`, with a warning, for the next
/// pass.
pub async fn apply(data_directory: &Path, policy: UpdateBackupPolicy) -> UpdateBackupPlan {
    let _folder = pre_update::FOLDER_LOCK.lock().await;
    let mut plan = plan(data_directory, policy).await;
    let mut removed = Vec::new();
    for file in std::mem::take(&mut plan.removable) {
        match tokio::fs::remove_file(&file.path).await {
            Ok(()) => removed.push(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, file = %file.path.display(), "an old backup before an update could not be removed");
                plan.kept.push(file);
            }
        }
    }
    plan.removable = removed;
    plan
}

/// The newest file stays while the update is not proven or its grace has not run out; behind a
/// proven update nothing older does. An unproven one keeps everything.
fn split(
    mut files: Vec<BackupFile>,
    policy: UpdateBackupPolicy,
) -> (Vec<BackupFile>, Vec<BackupFile>) {
    if !policy.proven || files.is_empty() {
        return (files, Vec::new());
    }
    files.sort_by(|left, right| left.stamp.cmp(&right.stamp));
    let Some(newest) = files.pop() else {
        return (Vec::new(), Vec::new());
    };
    let mut removable = files;
    if expired(&newest, policy) {
        removable.push(newest);
        (Vec::new(), removable)
    } else {
        (vec![newest], removable)
    }
}

/// Whether the newest file has outlived its grace. One without a readable time stays.
fn expired(file: &BackupFile, policy: UpdateBackupPolicy) -> bool {
    if policy.grace_days == 0 {
        return false;
    }
    let grace = Duration::from_secs(u64::from(policy.grace_days) * 24 * 60 * 60);
    file.modified
        .and_then(|modified| policy.now.duration_since(modified).ok())
        .is_some_and(|age| age > grace)
}

/// The files of one kind, by the names their writers give them; everything else is left out.
async fn list(data_directory: &Path, kind: BackupKind) -> Vec<BackupFile> {
    let folder = match kind {
        BackupKind::PreUpdateCopy | BackupKind::PreUpdateArchive => {
            pre_update::directory(data_directory)
        }
        BackupKind::PreMigrationCopy => data_directory.join(rd_db::pre_migration::DIRECTORY),
    };
    let Ok(mut entries) = tokio::fs::read_dir(&folder).await else {
        return Vec::new();
    };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let stamp = match kind {
            BackupKind::PreUpdateArchive => archive_stamp(&name),
            BackupKind::PreUpdateCopy | BackupKind::PreMigrationCopy => {
                rd_db::pre_migration::snapshot_stamp(&name)
            }
        };
        let Some(stamp) = stamp.map(str::to_owned) else {
            continue;
        };
        // A symlink is never one of ours to measure or remove.
        let Ok(metadata) = tokio::fs::symlink_metadata(entry.path()).await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        files.push(BackupFile {
            kind,
            path: entry.path(),
            size_bytes: metadata.len(),
            stamp,
            modified: metadata.modified().ok(),
        });
    }
    files
}

#[cfg(test)]
#[path = "update_retention_tests.rs"]
mod tests;

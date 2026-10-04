//! Switching an installation to a restored state (RD-160-03).
//!
//! The running service never replaces its own database. A restore stages the restored state
//! beside the live one and leaves a marker; the switch happens at the next start, before the
//! database is opened, by renames inside the data directory:
//!
//! 1. **Staged.** `restore-staged/` holds the database copy, `torrent-session/` and `torrents/`
//!    (both always present, empty when the backup had none), and `restore-pending.json` says
//!    `staged`. Nothing live has changed.
//! 2. **Switch** ([`switch`]). Every live item — the database, its `-wal` and `-shm`, the two
//!    torrent folders — is renamed into `restore-previous/`, and the staged one into its place.
//!    Each step looks at what is where, so a switch stopped anywhere is finished by the next
//!    start (crash point `restore.after_live_set_aside`). The marker then says `switched`.
//! 3. **First start.** The service opens the restored database. Once the start completed,
//!    [`finish`] removes the marker, then `restore-previous/` and what is left of the staging.
//!
//! **The previous installation stays startable until then.** A restored database that does not
//! open is put back by [`roll_back`] in the same start; a start that ended without reaching
//! [`finish`] leaves the marker at `switched`, and the next start rolls back rather than trying
//! the restored state again. A rolled-back state is kept in `restore-failed/` with the reason,
//! for the interface to report, until it is dismissed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_files::durable::{exists, remove_any, sync_directory};
use serde::{Deserialize, Serialize};

/// Below the data directory: the throwaway unpacks of previews and test restores.
pub const WORK_DIR: &str = "restore-work";
/// Below the data directory: uploaded archives, until used or the next start.
pub const UPLOADS_DIR: &str = "restore-uploads";
/// Below the data directory: the restored state waiting for the next start.
pub const STAGED_DIR: &str = "restore-staged";
/// Below the data directory: the live state set aside by the switch, until the first start.
pub const PREVIOUS_DIR: &str = "restore-previous";
/// Below the data directory: a restored state that did not start, and why.
pub const FAILED_DIR: &str = "restore-failed";
/// Below the data directory: the marker the start honours.
pub const MARKER: &str = "restore-pending.json";
/// Inside [`FAILED_DIR`]: why the restored state was put aside.
pub const FAILURE_FILE: &str = "outcome.json";
/// The database copy's name inside the staging, as the archive names it.
pub const STAGED_DATABASE: &str = crate::manifest::DATABASE_PART;
/// The torrent folders, by the names the torrent service gives them below the data directory.
pub const TORRENT_FOLDERS: [&str; 2] = ["torrent-session", "torrents"];

/// Where an installation's state lies: the data directory and the database file in it.
#[derive(Clone, Debug)]
pub struct Layout {
    data: PathBuf,
    database: PathBuf,
}

impl Layout {
    /// The layout of the installation whose database is `database`; its folder is the data
    /// directory, as everywhere else.
    #[must_use]
    pub fn new(database: &Path) -> Self {
        let data = database
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self {
            data,
            database: database.to_path_buf(),
        }
    }

    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    #[must_use]
    pub fn staged(&self) -> PathBuf {
        self.data.join(STAGED_DIR)
    }

    #[must_use]
    pub fn work(&self) -> PathBuf {
        self.data.join(WORK_DIR)
    }

    #[must_use]
    pub fn uploads(&self) -> PathBuf {
        self.data.join(UPLOADS_DIR)
    }

    #[must_use]
    pub fn previous(&self) -> PathBuf {
        self.data.join(PREVIOUS_DIR)
    }

    #[must_use]
    pub fn failed(&self) -> PathBuf {
        self.data.join(FAILED_DIR)
    }

    #[must_use]
    pub fn marker(&self) -> PathBuf {
        self.data.join(MARKER)
    }

    /// Every item the switch moves: its name inside the staging (`None`: it has no staged
    /// counterpart and only goes aside), and its live place.
    fn items(&self) -> Vec<(Option<String>, PathBuf)> {
        let file_name = self.database.file_name().map_or_else(
            || "rdownloader.sqlite3".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut items = vec![
            (None, self.data.join(format!("{file_name}-wal"))),
            (None, self.data.join(format!("{file_name}-shm"))),
            (Some(STAGED_DATABASE.to_owned()), self.database.clone()),
        ];
        for folder in TORRENT_FOLDERS {
            items.push((Some(folder.to_owned()), self.data.join(folder)));
        }
        items
    }
}

/// Where a restore stands.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Staged; the next start switches.
    Staged,
    /// Switched; the start that switched has not completed yet.
    Switched,
}

/// The marker: what is staged, and what a discard or a roll-back has to clean up.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PendingRestore {
    pub phase: Phase,
    pub staged_at: DateTime<Utc>,
    /// The archive's file name, for the interface.
    pub archive_name: String,
    /// When the backup was made, and by which version.
    pub backup_created_at: DateTime<Utc>,
    pub app_version: String,
    /// References this restore put into the secret store; removed again if it never starts.
    #[serde(default)]
    pub minted_secrets: Vec<String>,
}

/// Why a restored state was put aside.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FailedRestore {
    pub failed_at: DateTime<Utc>,
    pub reason: String,
    pub archive_name: Option<String>,
}

/// What the start found and did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cutover {
    /// No restore was waiting.
    Nothing,
    /// The restored state is in place; [`finish`] once the start completed.
    Switched(PendingRestore),
    /// The restored state was put aside and the previous one is back.
    RolledBack {
        pending: Option<PendingRestore>,
        reason: String,
    },
}

/// How long a rename waits for a handle that is still being closed (Windows only): a database
/// that failed to open closes its handle on SQLite's worker thread, shortly after
/// `Database::open` has returned the error, and the roll-back right after it met "used by
/// another process" (os error 32).
const RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

fn rename(from: &Path, to: &Path) -> Result<()> {
    rd_files::durable::rename(from, to, RELEASE_WAIT)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    rd_files::durable::write_atomically(path, bytes, RELEASE_WAIT)
}

fn write_marker(layout: &Layout, pending: &PendingRestore) -> Result<()> {
    write_atomically(&layout.marker(), &serde_json::to_vec_pretty(pending)?)
}

/// The marker, if a restore is waiting or switching.
///
/// # Errors
///
/// When the marker exists and cannot be read.
pub fn read_marker(layout: &Layout) -> Result<Option<PendingRestore>> {
    match fs::read(layout.marker()) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("read the restore marker")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("read the restore marker"),
    }
}

/// The record of a restored state that was put aside, if one is kept.
#[must_use]
pub fn read_failure(layout: &Layout) -> Option<FailedRestore> {
    let bytes = fs::read(layout.failed().join(FAILURE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Moves a finished staging folder into place and writes the marker: the service side's last
/// step. `prepared` must hold the database copy; the torrent folders are created when missing.
///
/// # Errors
///
/// `a restore is already waiting` when one is staged, or when a step fails; nothing is left
/// half-staged then.
pub fn stage(layout: &Layout, prepared: &Path, pending: &PendingRestore) -> Result<()> {
    anyhow::ensure!(
        read_marker(layout)?.is_none() && !exists(&layout.staged()),
        "a restore is already waiting for the next start"
    );
    anyhow::ensure!(
        prepared.join(STAGED_DATABASE).is_file(),
        "the prepared restore has no database"
    );
    for folder in TORRENT_FOLDERS {
        fs::create_dir_all(prepared.join(folder))?;
    }
    rename(prepared, &layout.staged())?;
    sync_directory(layout.data());
    let marker = PendingRestore {
        phase: Phase::Staged,
        ..pending.clone()
    };
    if let Err(error) = write_marker(layout, &marker) {
        let _ = remove_any(&layout.staged());
        return Err(error);
    }
    Ok(())
}

/// Removes a restore that has not switched yet and returns its marker, so the caller can take
/// its secrets out of the store again. A kept failure record goes with it.
///
/// # Errors
///
/// When the restore already switched (only a start can undo that), or a removal fails.
pub fn discard(layout: &Layout) -> Result<Option<PendingRestore>> {
    let pending = read_marker(layout)?;
    anyhow::ensure!(
        pending
            .as_ref()
            .is_none_or(|pending| pending.phase == Phase::Staged),
        "the restore is being switched; only the next start can finish or undo it"
    );
    remove_any(&layout.marker())?;
    remove_any(&layout.staged())?;
    remove_any(&layout.failed())?;
    Ok(pending)
}

/// The switch itself: sets every live item aside and puts the staged one in its place, then
/// marks the marker `switched`. Idempotent — a switch stopped anywhere is finished by running
/// it again. What [`apply_pending`] runs; public for the crash matrix.
///
/// # Errors
///
/// When a rename fails or the items are in a state no switch leaves them in.
pub fn switch(layout: &Layout, pending: &PendingRestore) -> Result<()> {
    let staged = layout.staged();
    let previous = layout.previous();
    fs::create_dir_all(&previous)?;
    for (staged_name, live) in layout.items() {
        let name = live
            .file_name()
            .context("a live item has no name")?
            .to_owned();
        let aside = previous.join(&name);
        let incoming = staged_name.map(|staged_name| staged.join(staged_name));
        let arriving = incoming.as_deref().is_some_and(exists);
        // An item without a staged counterpart is set aside once; a staged one only while it
        // still waits in the staging — once it left it, it is the live one.
        if exists(&live) && (arriving || incoming.is_none()) {
            anyhow::ensure!(
                !exists(&aside),
                "{} and its previous copy both exist; the switch cannot tell them apart",
                live.display()
            );
            rename(&live, &aside)?;
            rd_core::failpoint!("restore.after_live_set_aside", || anyhow::anyhow!(
                "crash point"
            ));
        }
        if let Some(incoming) = incoming.filter(|incoming| exists(incoming)) {
            rename(&incoming, &live)?;
        }
    }
    sync_directory(layout.data());
    write_marker(
        layout,
        &PendingRestore {
            phase: Phase::Switched,
            ..pending.clone()
        },
    )
}

/// Puts the previous installation back and keeps the restored state in `restore-failed/` with
/// `reason`. Works from any point of a switch.
///
/// # Errors
///
/// When a rename fails; the start must then not go on.
pub fn roll_back(layout: &Layout, reason: &str) -> Result<Option<PendingRestore>> {
    let pending = read_marker(layout).ok().flatten();
    let failed = layout.failed();
    remove_any(&failed)?;
    fs::create_dir_all(&failed)?;
    let previous = layout.previous();
    let staged = layout.staged();
    let staging = exists(&staged);
    // An item came out of the staging when the staging is there and the item no longer is. A
    // missing staging proves nothing: an unreadable marker may stand for a switch that never
    // began, and then every live item is still the installation's own.
    let came_out = |staged_name: &str| staging && !exists(&staged.join(staged_name));
    let database_name = layout
        .database
        .file_name()
        .context("the database has no file name")?
        .to_owned();
    // The journal files have no staged counterpart and go first; once the database itself was
    // touched, any journal beside it belongs to the restored database.
    let database_touched = exists(&previous.join(&database_name)) || came_out(STAGED_DATABASE);
    for (staged_name, live) in layout.items() {
        let name = live
            .file_name()
            .context("a live item has no name")?
            .to_owned();
        let aside = previous.join(&name);
        let live_is_restored = exists(&aside)
            || match staged_name.as_deref() {
                Some(staged_name) => came_out(staged_name),
                None => database_touched,
            };
        if exists(&live) && live_is_restored {
            rename(&live, &failed.join(&name))?;
        }
        if exists(&aside) {
            rename(&aside, &live)?;
        }
    }
    sync_directory(layout.data());
    let record = FailedRestore {
        failed_at: Utc::now(),
        reason: reason.to_owned(),
        archive_name: pending.as_ref().map(|pending| pending.archive_name.clone()),
    };
    write_atomically(
        &failed.join(FAILURE_FILE),
        &serde_json::to_vec_pretty(&record)?,
    )?;
    remove_any(&layout.marker())?;
    remove_any(&staged)?;
    remove_any(&previous)?;
    Ok(pending)
}

/// What the start runs before it opens the database: empties the throwaway folders, switches
/// a staged restore, and rolls back one whose first start never completed.
///
/// # Errors
///
/// Only when a roll-back itself fails: then the installation is in no state to start.
pub fn apply_pending(layout: &Layout) -> Result<Cutover> {
    for folder in [layout.work(), layout.uploads()] {
        if let Err(error) = remove_any(&folder) {
            tracing::warn!(%error, "a restore folder could not be emptied");
        }
    }
    let pending = match read_marker(layout) {
        Ok(pending) => pending,
        Err(error) => {
            let reason = format!("the restore marker is unreadable: {error:#}");
            let pending = roll_back(layout, &reason)?;
            return Ok(Cutover::RolledBack { pending, reason });
        }
    };
    let Some(pending) = pending else {
        // A staging without its marker was never complete; a previous state without one is
        // what a completed restore left when it stopped while cleaning up.
        for folder in [layout.staged(), layout.previous()] {
            if let Err(error) = remove_any(&folder) {
                tracing::warn!(%error, "a leftover restore folder could not be removed");
            }
        }
        return Ok(Cutover::Nothing);
    };
    match pending.phase {
        Phase::Staged => match switch(layout, &pending) {
            Ok(()) => {
                tracing::info!(archive = %pending.archive_name, "the restored state is in place");
                Ok(Cutover::Switched(pending))
            }
            Err(error) => {
                let reason = format!("the switch to the restored state failed: {error:#}");
                tracing::error!(%reason, "the previous installation is put back");
                let pending = roll_back(layout, &reason)?;
                Ok(Cutover::RolledBack { pending, reason })
            }
        },
        Phase::Switched => {
            let reason = "the first start with the restored state did not complete".to_owned();
            tracing::error!(%reason, "the previous installation is put back");
            let pending = roll_back(layout, &reason)?;
            Ok(Cutover::RolledBack { pending, reason })
        }
    }
}

/// The first start with the restored state completed: the marker goes first, which commits
/// the restore, then the previous state and the empty staging.
///
/// # Errors
///
/// When the marker cannot be removed; leftovers are only warned about, the next start removes
/// them.
pub fn finish(layout: &Layout) -> Result<()> {
    remove_any(&layout.marker())?;
    sync_directory(layout.data());
    for folder in [layout.previous(), layout.staged()] {
        if let Err(error) = remove_any(&folder) {
            tracing::warn!(%error, "a finished restore's folder could not be removed");
        }
    }
    Ok(())
}

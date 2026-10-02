//! What an update could not remove yet (RD-180-02, live finding 2026-10-02): the leftovers of an
//! earlier update are moved into `<install>/.trash/` and removed from there, and whatever the
//! system still holds stays in the trash for a later sweep instead of failing the update.
//!
//! The case it exists for: Windows lets a running executable be moved but not deleted. The
//! portable switch moves `rdownloader-capture.exe` into `.previous/` while the capture agent runs
//! from it, so `.previous/` cannot be removed until that agent ends — and every later update used
//! to stop at once with `update.unpack_failed` ("remove …\.previous: access denied"). The trash
//! sits beside the program files, on their volume, where such a move works.
//!
//! A leftover is moved whole where the system lets it; a folder it refuses to move (on Windows one
//! that holds a running program) is moved entry by entry. A stop anywhere in between leaves the
//! program files untouched, the leftover partly in the trash and partly where it was, and the next
//! update or start goes on from there (crash point `update.after_leftover_set_aside`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use super::{exists, remove_any, rename};

/// Beside the program files: the leftovers of earlier updates that could not be removed yet.
pub const TRASH_DIR: &str = ".trash";

/// `<install>/.trash`.
#[must_use]
pub fn trash_dir(install: &Path) -> PathBuf {
    install.join(TRASH_DIR)
}

/// Moves `leftover`, a folder or file inside `install`, into the trash, then removes from the
/// trash what can go now ([`sweep`]). Nothing happens when it does not exist.
///
/// # Errors
///
/// When the leftover can be moved neither whole nor entry by entry; what did move stays in the
/// trash, the rest where it was.
pub fn discard(leftover: &Path, install: &Path) -> Result<()> {
    if !exists(leftover) {
        return Ok(());
    }
    let name = leftover
        .file_name()
        .with_context(|| format!("{} has no name", leftover.display()))?;
    let batch = new_batch(install)?;
    set_aside(leftover, &batch.join(name))?;
    rd_core::failpoint!("update.after_leftover_set_aside", || anyhow::anyhow!(
        "crash point"
    ));
    sweep(install);
    Ok(())
}

/// Removes everything in the trash of `install` that the system lets go; the rest stays for the
/// next sweep, and the trash itself goes once it is empty. Never fails: what stays is logged.
pub fn sweep(install: &Path) {
    let trash = trash_dir(install);
    let entries = match fs::read_dir(&trash) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(%error, trash = %trash.display(), "the update trash cannot be read");
            return;
        }
    };
    let mut emptied = true;
    for entry in entries.flatten() {
        if let Err(error) = remove_any(&entry.path()) {
            emptied = false;
            tracing::warn!(
                error = format!("{error:#}"),
                "an update leftover is still in use and stays in the trash until a later start"
            );
        }
    }
    if emptied {
        let _ = fs::remove_dir(&trash);
    }
}

/// A new, empty folder in the trash, named by the time, for one leftover.
fn new_batch(install: &Path) -> Result<PathBuf> {
    let trash = trash_dir(install);
    fs::create_dir_all(&trash).with_context(|| format!("create {}", trash.display()))?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
    for attempt in 0..100_u32 {
        let batch = trash.join(format!("{stamp}-{attempt}"));
        match fs::create_dir(&batch) {
            Ok(()) => return Ok(batch),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("create {}", batch.display()));
            }
        }
    }
    bail!("no free name in {}", trash.display())
}

/// Moves `from` to `to`: whole when the system lets it, otherwise — a folder holding a running
/// program on Windows — every entry on its own, then the emptied folder is removed.
fn set_aside(from: &Path, to: &Path) -> Result<()> {
    // One attempt only: the folder of a running program is refused at once and for as long as
    // it runs, and the entries below move without waiting for it.
    let Err(refused) = fs::rename(from, to) else {
        return Ok(());
    };
    let is_folder = fs::symlink_metadata(from).is_ok_and(|metadata| metadata.is_dir());
    if !is_folder {
        // A single file: the rename that waits out a handle still being closed.
        return rename(from, to).with_context(|| format!("first attempt: {refused}"));
    }
    tracing::debug!(
        error = %refused,
        from = %from.display(),
        "a leftover folder is moved entry by entry"
    );
    fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("list {}", from.display()))? {
        let entry = entry.with_context(|| format!("list {}", from.display()))?;
        set_aside(&entry.path(), &to.join(entry.file_name()))?;
    }
    fs::remove_dir(from).with_context(|| format!("remove {}", from.display()))
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;

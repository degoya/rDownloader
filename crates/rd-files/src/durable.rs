//! File-system steps that have to survive a stop half-way: the switch of a restore
//! (`rd-backup`) and of a portable update (`rd-update`) are built from them (DB-07), and the
//! vault syncs its folder with [`sync_directory`] (DB-08).
//!
//! Blocking calls: both callers run them before or outside the runtime's busy work, where a
//! short wait on the disk is what they want.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// How often [`rename`] tries again while a handle is still being closed.
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Whether anything is at `path`, a dangling link included.
#[must_use]
pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Removes the file, link or directory tree at `path`; nothing there is no error.
///
/// # Errors
///
/// When something is there and cannot be removed.
pub fn remove_any(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .with_context(|| format!("remove {}", path.display()))
}

/// Renames `from` to `to`, retrying a sharing or access violation on Windows for up to
/// `release_wait`.
///
/// On Windows a file stays locked until its last handle is closed: a database that failed to
/// open closes its handle on SQLite's worker thread shortly after the error came back, a process
/// that just ended or a virus scanner looking at a new executable holds its files a moment
/// longer. Elsewhere the first answer is the answer.
///
/// # Errors
///
/// When the rename fails, or still fails once `release_wait` is up.
pub fn rename(from: &Path, to: &Path, release_wait: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        match fs::rename(from, to) {
            Err(error)
                if cfg!(windows)
                    && matches!(error.raw_os_error(), Some(5 | 32))
                    && started.elapsed() < release_wait =>
            {
                std::thread::sleep(RETRY_INTERVAL);
            }
            result => {
                return result
                    .with_context(|| format!("move {} to {}", from.display(), to.display()));
            }
        }
    }
}

/// Makes a rename or a new file in `directory` durable. On Windows a directory cannot be opened
/// this way, and `MoveFileEx` without write-through is what there is.
pub fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    let _ = fs::File::open(directory).and_then(|handle| handle.sync_all());
    #[cfg(not(unix))]
    let _ = directory;
}

/// Replaces `path` with `bytes` so a stop leaves either the old content or the new one: written
/// and synced beside it under `<name>.tmp`, renamed over it ([`rename`], with `release_wait`),
/// and the folder synced.
///
/// # Errors
///
/// When the temporary cannot be written or renamed.
pub fn write_atomically(path: &Path, bytes: &[u8], release_wait: Duration) -> Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    let temporary = std::path::PathBuf::from(name);
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    rename(&temporary, path, release_wait)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomically_replaces_and_leaves_no_temporary() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("marker.json");
        write_atomically(&path, b"{\"a\":1}", Duration::ZERO).expect("first");
        write_atomically(&path, b"{\"a\":2}", Duration::ZERO).expect("second");
        assert_eq!(fs::read(&path).expect("read"), b"{\"a\":2}");
        assert!(!exists(&directory.path().join("marker.json.tmp")));
    }

    #[test]
    fn remove_any_takes_files_trees_and_nothing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tree = directory.path().join("tree");
        fs::create_dir_all(tree.join("inner")).expect("tree");
        fs::write(tree.join("inner/file"), b"x").expect("file");
        let file = directory.path().join("file");
        fs::write(&file, b"x").expect("file");
        remove_any(&tree).expect("tree");
        remove_any(&file).expect("file");
        remove_any(&directory.path().join("absent")).expect("absent");
        assert!(!exists(&tree) && !exists(&file));
    }
}

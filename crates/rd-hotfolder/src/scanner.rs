//! One pass over a watched folder: which files are candidates, when one is stable, and where
//! it goes once it was handed over.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{Result, bail};
use rd_core::HotFolderConfig;
use sha2::{Digest, Sha256};

use crate::{
    DUPLICATE_CODE, DuplicateIntake, FailedIntake, HotFolderIntake, IntakeSink, MAX_INTAKE_BYTES,
};

pub(crate) struct Scanner<'a> {
    pub(crate) config: HotFolderConfig,
    pub(crate) root: PathBuf,
    pub(crate) processed: PathBuf,
    pub(crate) failed: PathBuf,
    pub(crate) sink: Arc<dyn IntakeSink>,
    pub(crate) stability: Duration,
    pub(crate) observed: HashMap<PathBuf, Observation>,
    pub(crate) imported: &'a mut HashSet<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct Observation {
    length: u64,
    modified: SystemTime,
    unchanged_since: tokio::time::Instant,
}

impl Scanner<'_> {
    /// One pass over the folder. A file that cannot be handled is logged and left where it is.
    ///
    /// Propagating a per-file error from here ended the watcher task for good: a file that
    /// vanished between the stability check and the read, a full `processed` directory or one
    /// unreadable entry stopped the folder importing anything until the service was
    /// restarted, with nothing but a dead `JoinHandle` to say so.
    pub(crate) async fn scan(&mut self) -> Result<()> {
        for path in list_files(&self.root, self.config.recursive).await? {
            if let Err(error) = self.inspect(path.clone()).await {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "hotfolder could not process this file; it stays for the next pass"
                );
            }
        }
        Ok(())
    }

    pub(crate) async fn inspect(&mut self, path: PathBuf) -> Result<()> {
        if !is_candidate(&path)
            || path.starts_with(&self.processed)
            || path.starts_with(&self.failed)
        {
            return Ok(());
        }
        let Ok(metadata) = tokio::fs::metadata(&path).await else {
            self.observed.remove(&path);
            return Ok(());
        };
        if !metadata.is_file() || metadata.len() > MAX_INTAKE_BYTES {
            return Ok(());
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let now = tokio::time::Instant::now();
        let stable = self.observed.get(&path).is_some_and(|previous| {
            previous.length == metadata.len()
                && previous.modified == modified
                && now.duration_since(previous.unchanged_since) >= self.stability
        });
        if !stable {
            let unchanged_since = self
                .observed
                .get(&path)
                .filter(|previous| {
                    previous.length == metadata.len() && previous.modified == modified
                })
                .map_or(now, |previous| previous.unchanged_since);
            self.observed.insert(
                path,
                Observation {
                    length: metadata.len(),
                    modified,
                    unchanged_since,
                },
            );
            return Ok(());
        }
        self.import(path).await
    }

    async fn import(&mut self, path: PathBuf) -> Result<()> {
        self.observed.remove(&path);
        let content = tokio::fs::read(&path).await?;
        let sha256 = hex::encode(Sha256::digest(&content));
        if self.imported.contains(&sha256) {
            let processed_path = unique_destination(&self.processed, &path);
            tracing::warn!(
                code = DUPLICATE_CODE,
                path = %path.display(),
                processed_path = %processed_path.display(),
                "hotfolder file repeats one already imported; moved without a second import"
            );
            self.sink
                .record_duplicate(DuplicateIntake {
                    source_path: path.clone(),
                    sha256: sha256.clone(),
                    processed_path: processed_path.clone(),
                })
                .await;
            move_aside(&path, &processed_path).await?;
            self.imported.remove(&sha256);
            return Ok(());
        }
        let intake = HotFolderIntake {
            source_path: path.clone(),
            sha256: sha256.clone(),
            content,
            mode: self.config.import_mode,
            category_id: self.config.category_id,
        };
        let mut failure = None;
        let destination = match self.sink.submit(intake).await {
            Ok(()) => {
                self.imported.insert(sha256.clone());
                unique_destination(&self.processed, &path)
            }
            Err(error) => {
                let failed_path = unique_destination(&self.failed, &path);
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    failed_path = %failed_path.display(),
                    "hotfolder import failed; file moved to failed directory"
                );
                failure = Some(FailedIntake {
                    source_path: path.clone(),
                    sha256: sha256.clone(),
                    failed_path: failed_path.clone(),
                    reason: reason(&error),
                });
                failed_path
            }
        };
        // Reported before the move, and deliberately so: a move that fails takes the whole
        // `import` with it, and the reason is worth more than the certainty that the file is
        // already at `failed_path`. A file left behind is picked up by the next pass and
        // reported again, which updates the record rather than adding a second one.
        if let Some(failure) = failure {
            self.sink.record_failure(failure).await;
        }
        move_aside(&path, &destination).await?;
        // Out of flight: the file is gone from the folder, so the same content dropped again
        // is a deliberate second drop and is imported (INTAKE-16).
        self.imported.remove(&sha256);
        Ok(())
    }
}

/// Every candidate file below `root`.
///
/// The root has to be readable - if it is not, the folder itself failed and the watcher starts
/// over after a backoff. A single sub-directory that is not is skipped instead, so one
/// permission problem somewhere in the tree does not stop the whole folder.
async fn list_files(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if directory == root => return Err(error.into()),
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %directory.display(),
                    "hotfolder skipped a directory it cannot read"
                );
                continue;
            }
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            if kind.is_dir() && recursive {
                directories.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

/// Moves a handled file to `destination`, a name [`unique_destination`] found free.
///
/// Across devices the copy is verified before the original goes, streamed rather than read
/// into memory, and a file that took the name in the meantime is never overwritten.
async fn move_aside(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    rd_files::verified_move_file(source, destination).await?;
    Ok(())
}

/// The error chain as one bounded line.
///
/// Bounded because this is stored and shown: an error that carries a parser dump or a server
/// answer would otherwise put an unbounded blob into the database and into the list that shows
/// it. Newlines collapse so the reason stays one line in a row that has room for one.
pub(crate) fn reason(error: &anyhow::Error) -> String {
    let flattened = format!("{error:#}")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match flattened.char_indices().nth(MAX_REASON_CHARS) {
        Some((index, _)) => format!("{}...", &flattened[..index]),
        None => flattened,
    }
}

pub(crate) const MAX_REASON_CHARS: usize = 500;

fn unique_destination(directory: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("import.nzb");
    rd_files::collision_free_path(directory, name)
}

pub(crate) fn destination_path(root: &Path, configured: &str) -> Result<PathBuf> {
    let path = Path::new(configured);
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("hotfolder destination must be a non-empty relative path");
    }
    if path
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("hotfolder destination contains a forbidden path component");
    }
    Ok(root.join(path))
}

pub(crate) fn checked_destination(root: &Path, path: PathBuf) -> Result<PathBuf> {
    let canonical = dunce::canonicalize(&path)?;
    if !canonical.starts_with(root) {
        bail!("hotfolder destination escapes through a symlink");
    }
    Ok(canonical)
}

pub(crate) fn dunce_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        bail!("hotfolder path is empty");
    }
    Ok(path)
}

/// Extensions a watched folder picks up.
///
/// Deliberately no `.txt`: a link list is a container the interface accepts on upload, but a
/// watched folder is somewhere people also keep notes, and picking up a README to announce it
/// holds no links — then moving it aside — is not a trade worth making for a format that is
/// one paste away anyway.
const CANDIDATE_EXTENSIONS: [&str; 5] = ["nzb", "torrent", "dlc", "ccf", "rsdf"];

pub(crate) fn is_candidate(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            CANDIDATE_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
                && !name.starts_with('.')
                && !name.ends_with('~')
                && !name.ends_with(".part")
                && !name.ends_with(".tmp")
        })
}

//! Moving a torrent's files to another folder without adding it anew (RD-1100-10).
//!
//! librqbit 9.0.1 fixes a torrent's folder when the torrent is added and has no call that
//! changes it, live or paused. A move therefore takes the torrent out of the session with its
//! files kept, moves the files, points the package at the new folder and adds the torrent again
//! there; a seed hashes its data once at the new place and seeds on from it.
//!
//! The move is journalled in the row's torrent state ([`TorrentRelocation`]) before the first
//! file moves. Every file is placed verified ([`rd_files::place_verified`]): within one
//! filesystem a rename, across two a copy whose SHA-256 is compared before it takes its name,
//! with the original kept. The originals go only after the package names the new folder, so
//! that switch decides the outcome, and every state a stop can leave is one the next start
//! resolves from the package alone:
//!
//! | Stopped | The package names | The next start |
//! | --- | --- | --- |
//! | before or while the files are placed | the old folder | takes it back: renamed files return, copies and temporary copies go |
//! | after the switch, before the release | the new folder | finishes it: originals still there go |
//!
//! A move that fails while it runs is taken back the same way at once, and the reason is kept on
//! the row ([`rd_core::TorrentJobState::relocation_error`]).

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use rd_core::{DownloadId, DownloadState, PackageId, TorrentJobState, TorrentRelocation};

use crate::TorrentService;

/// A move that is journalled.
struct Relocation {
    id: DownloadId,
    package_id: PackageId,
    journal: TorrentRelocation,
    /// The torrent's files, relative to the package folder.
    files: Vec<PathBuf>,
}

impl Relocation {
    fn source_folder(&self) -> &Path {
        Path::new(&self.journal.from)
    }

    fn target_folder(&self) -> &Path {
        Path::new(&self.journal.to)
    }
}

impl TorrentService {
    /// Bytes of one torrent's files that are on disk in its package folder: what a move to
    /// another filesystem has to copy.
    pub async fn relocation_bytes(&self, id: DownloadId) -> Result<u64> {
        let file = self
            .inner
            .database
            .get_download(id)
            .await?
            .context("download not found")?;
        let package = self
            .inner
            .database
            .get_package(file.package_id)
            .await?
            .context("package not found")?;
        let from = Path::new(&package.destination);
        let mut bytes = 0_u64;
        for relative in torrent_files(&self.job_state(id).await)? {
            if let Ok(metadata) = tokio::fs::metadata(from.join(relative)).await {
                bytes = bytes.saturating_add(metadata.len());
            }
        }
        Ok(bytes)
    }

    /// Journals the move of one torrent's files into the package folder `to`, takes the torrent
    /// out of the session and carries the move out in the background.
    ///
    /// Returns once the move is recorded; its outcome shows on the row — the package's new
    /// folder, or [`rd_core::TorrentJobState::relocation_error`] when it was taken back.
    pub async fn start_relocation(&self, id: DownloadId, to: PathBuf) -> Result<()> {
        let relocation = self.begin_relocation(id, to).await?;
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(error) = service.carry_out(relocation).await {
                tracing::warn!(download_id = %id, %error, "torrent move did not complete");
            }
        });
        Ok(())
    }

    /// [`Self::start_relocation`] start to finish, for the tests.
    #[cfg(test)]
    pub(crate) async fn relocate(&self, id: DownloadId, to: PathBuf) -> Result<()> {
        let relocation = self.begin_relocation(id, to).await?;
        self.carry_out(relocation).await
    }

    /// Whether a move of this torrent's files runs in this process right now.
    pub async fn is_relocating(&self, id: DownloadId) -> bool {
        self.inner.relocating.lock().await.contains(&id)
    }

    async fn begin_relocation(&self, id: DownloadId, to: PathBuf) -> Result<Relocation> {
        anyhow::ensure!(
            self.inner.relocating.lock().await.insert(id),
            "a move of this torrent is under way already"
        );
        let begun = self.journal_relocation(id, to).await;
        if begun.is_err() {
            self.inner.relocating.lock().await.remove(&id);
            // Nothing moved; a seed that already left the session takes it up again.
            self.resume_if_seeding(id).await;
        }
        begun
    }

    async fn journal_relocation(&self, id: DownloadId, to: PathBuf) -> Result<Relocation> {
        let file = self
            .inner
            .database
            .get_download(id)
            .await?
            .context("download not found")?;
        let package = self
            .inner
            .database
            .get_package(file.package_id)
            .await?
            .context("package not found")?;
        let files = torrent_files(&self.job_state(id).await)?;
        // Whatever is in the new folder would be mistaken for the torrent's when a move is
        // taken back; the REST layer refuses such a target before it gets here.
        anyhow::ensure!(folder_is_free(&to).await, "the target folder is not empty");
        // Out of the session with its files kept, and out of the registry with it, so neither
        // the supervisor nor the statistics reach for a torrent that is not there. First, so
        // that no seeding pass writes the torrent state over the journal below.
        self.forget(id).await;
        if file.state == DownloadState::Seeding {
            // The torrent does not seed while its files move.
            self.close_seed_clock(id).await;
        }
        let relocation = Relocation {
            id,
            package_id: package.id,
            journal: TorrentRelocation {
                from: package.destination.clone(),
                to: to.to_string_lossy().into_owned(),
                started_at: chrono::Utc::now(),
            },
            files,
        };
        // Durable before anything moves: it is what tells the next start a move was under way.
        self.write_journal(&relocation, true).await?;
        Ok(relocation)
    }

    /// Writes the journal into the row's torrent state; `fresh` also clears the reason the
    /// previous move was taken back.
    ///
    /// The state is one blob that every torrent control writes as a whole, so a write that read
    /// it before the journal was there can drop it again. The journal is therefore written once
    /// more right before the commit, the step after which losing it would matter.
    async fn write_journal(&self, relocation: &Relocation, fresh: bool) -> Result<()> {
        let mut state = self.job_state(relocation.id).await;
        if !fresh && state.relocation.as_ref() == Some(&relocation.journal) {
            return Ok(());
        }
        state.relocation = Some(relocation.journal.clone());
        if fresh {
            state.relocation_error = None;
        }
        self.inner
            .database
            .set_download_torrent_state(relocation.id, state)
            .await
            .context("record the move")
    }

    async fn carry_out(&self, relocation: Relocation) -> Result<()> {
        let result = self.run_relocation(&relocation).await;
        self.inner.relocating.lock().await.remove(&relocation.id);
        result
    }

    async fn run_relocation(&self, relocation: &Relocation) -> Result<()> {
        let committed = match place_all(relocation).await {
            Ok(()) => {
                rd_core::failpoint!("torrent.before_relocation_commit", || anyhow::anyhow!(
                    "crash point: torrent.before_relocation_commit"
                ));
                match self.write_journal(relocation, false).await {
                    Ok(()) => self.commit(relocation).await,
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        if let Err(error) = committed {
            if let Err(settling) = self.settle(relocation, &format!("{error:#}")).await {
                // The journal stays, so the next start settles it again.
                tracing::warn!(download_id = %relocation.id, error = %settling, "torrent move could not be settled yet");
            }
            self.resume_if_seeding(relocation.id).await;
            return Err(error);
        }
        rd_core::failpoint!("torrent.after_relocation_commit", || anyhow::anyhow!(
            "crash point: torrent.after_relocation_commit"
        ));
        self.finish(relocation).await?;
        self.resume_if_seeding(relocation.id).await;
        tracing::info!(download_id = %relocation.id, to = %relocation.journal.to, "torrent moved");
        Ok(())
    }

    /// Points the package at the new folder: the step after which the move counts as done.
    async fn commit(&self, relocation: &Relocation) -> Result<()> {
        let switched = self
            .inner
            .database
            .switch_package_destination(
                relocation.package_id,
                relocation.journal.from.clone(),
                relocation.journal.to.clone(),
            )
            .await?;
        anyhow::ensure!(
            switched,
            "the package no longer names the folder the move started from"
        );
        Ok(())
    }

    /// Ends a move that did not run to its end the way the package says it went: finished when
    /// it already names the new folder, taken back with `reason` otherwise.
    ///
    /// Read from the package rather than from the error at hand, because a commit whose answer
    /// was lost may still have been written.
    async fn settle(&self, relocation: &Relocation, reason: &str) -> Result<()> {
        let committed = self
            .inner
            .database
            .get_package(relocation.package_id)
            .await?
            .is_some_and(|package| package.destination == relocation.journal.to);
        if committed {
            self.finish(relocation).await
        } else {
            self.take_back(relocation, reason).await
        }
    }

    /// Ends the move with every file at the new place exactly once.
    async fn finish(&self, relocation: &Relocation) -> Result<()> {
        for relative in &relocation.files {
            let source = relocation.source_folder().join(relative);
            let target = relocation.target_folder().join(relative);
            if exists(&source).await? {
                if exists(&target).await? {
                    // The original of a copy that verified before it took its name.
                    discard(&source).await?;
                } else {
                    // Never placed, yet the package names the new folder: placed now.
                    create_parent(&target).await?;
                    let placed = rd_files::place_verified(&source, &target).await?;
                    rd_files::release_source(&source, &placed).await?;
                }
            }
            discard(&rd_files::move_temporary_of(&target)).await?;
        }
        remove_empty_folders(relocation.source_folder(), &relocation.files).await;
        let mut state = self.job_state(relocation.id).await;
        state.relocation = None;
        state.relocation_error = None;
        self.inner
            .database
            .set_download_torrent_state(relocation.id, state)
            .await?;
        // A session entry restored from a list written before the move would seed from the
        // old folder; the next add starts from the package's.
        self.forget(relocation.id).await;
        Ok(())
    }

    /// Ends the move with every file at the old place exactly once and the reason kept.
    async fn take_back(&self, relocation: &Relocation, reason: &str) -> Result<()> {
        for relative in &relocation.files {
            let source = relocation.source_folder().join(relative);
            let target = relocation.target_folder().join(relative);
            discard(&rd_files::move_temporary_of(&target)).await?;
            if !exists(&target).await? {
                continue;
            }
            if exists(&source).await? {
                // A copy beside the original that stayed: the original is the file. Only a
                // true copy goes; anything else under that name is left as it is.
                if same_content(&source, &target).await? {
                    discard(&target).await?;
                } else {
                    tracing::warn!(path = %target.display(), "a file that is not the torrent's copy was left in the move's target folder");
                }
            } else {
                // Renamed: it goes back the way it came.
                create_parent(&source).await?;
                rd_files::move_file(&target, &source).await?;
            }
        }
        remove_empty_folders(relocation.target_folder(), &relocation.files).await;
        let mut state = self.job_state(relocation.id).await;
        state.relocation = None;
        state.relocation_error = Some(reason.to_owned());
        self.inner
            .database
            .set_download_torrent_state(relocation.id, state)
            .await?;
        tracing::warn!(download_id = %relocation.id, reason, "torrent move taken back");
        Ok(())
    }

    /// Finishes or takes back every move a stop interrupted, before the seeds are taken up
    /// again: finished when the package already names the new folder, taken back otherwise.
    pub(crate) async fn resolve_relocations(&self) -> Result<()> {
        for (id, state) in self.inner.database.all_download_torrent_states().await? {
            let Some(journal) = state.relocation.clone() else {
                continue;
            };
            let Some(file) = self.inner.database.get_download(id).await? else {
                continue;
            };
            let Some(package) = self.inner.database.get_package(file.package_id).await? else {
                continue;
            };
            let files = match torrent_files(&state) {
                Ok(files) => files,
                Err(error) => {
                    tracing::warn!(download_id = %id, %error, "interrupted torrent move cannot be resolved");
                    continue;
                }
            };
            let relocation = Relocation {
                id,
                package_id: package.id,
                journal,
                files,
            };
            if let Err(error) = self
                .settle(&relocation, "the move was interrupted by a stop")
                .await
            {
                tracing::warn!(download_id = %id, %error, "interrupted torrent move could not be resolved");
            }
        }
        Ok(())
    }

    /// Adds a seeding row's torrent again from the folder its package names.
    async fn resume_if_seeding(&self, id: DownloadId) {
        let Ok(Some(file)) = self.inner.database.get_download(id).await else {
            return;
        };
        if file.state != DownloadState::Seeding {
            return;
        }
        let Ok(Some(package)) = self.inner.database.get_package(file.package_id).await else {
            return;
        };
        if let Err(error) = crate::seeding::resume(self, &file, &package).await {
            tracing::warn!(download_id = %id, %error, "seed could not be taken up again after its move");
        }
    }
}

/// Puts every file of the torrent that is on disk at the new place, originals kept where a
/// copy was needed.
async fn place_all(relocation: &Relocation) -> Result<()> {
    for relative in &relocation.files {
        let source = relocation.source_folder().join(relative);
        // Deselected files and padding were never written.
        if !exists(&source).await? {
            continue;
        }
        let target = relocation.target_folder().join(relative);
        create_parent(&target).await?;
        rd_files::place_verified(&source, &target)
            .await
            .with_context(|| format!("move {}", relative.display()))?;
    }
    Ok(())
}

/// The torrent's files relative to its package folder, refused whole if one would leave it.
fn torrent_files(state: &TorrentJobState) -> Result<Vec<PathBuf>> {
    let metadata = state
        .metadata
        .as_ref()
        .context("the torrent's file list is not known yet")?;
    metadata
        .files
        .iter()
        .map(|file| {
            relative_path(&file.path).with_context(|| {
                format!("unsafe file path in the torrent: {}", file.display_path())
            })
        })
        .collect()
}

/// Joins path components that are each one plain name; `None` for anything else.
fn relative_path(components: &[String]) -> Option<PathBuf> {
    if components.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    for component in components {
        let mut parts = Path::new(component).components();
        if !matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return None;
        }
        path.push(component);
    }
    Some(path)
}

/// Removes the folders the torrent's files lived in, deepest first, and `root` itself — each
/// only when it is empty, so nothing that is not the torrent's goes with them.
async fn remove_empty_folders(root: &Path, files: &[PathBuf]) {
    let folders: BTreeSet<&Path> = files
        .iter()
        .flat_map(|file| file.ancestors().skip(1))
        .filter(|folder| !folder.as_os_str().is_empty())
        .collect();
    let mut folders: Vec<&Path> = folders.into_iter().collect();
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.components().count()));
    for folder in folders {
        let _ = tokio::fs::remove_dir(root.join(folder)).await;
    }
    let _ = tokio::fs::remove_dir(root).await;
}

/// Whether `path` is no folder yet or an empty one.
async fn folder_is_free(path: &Path) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => matches!(entries.next_entry().await, Ok(None)),
        Err(_) => true,
    }
}

/// Whether two files hold the same bytes: the length first, the SHA-256 only when it matches.
async fn same_content(first: &Path, second: &Path) -> Result<bool> {
    let (left, right) = (
        tokio::fs::metadata(first).await?,
        tokio::fs::metadata(second).await?,
    );
    if left.len() != right.len() {
        return Ok(false);
    }
    let algorithm = rd_core::ChecksumAlgorithm::Sha256;
    Ok(rd_files::compute_checksum(first, algorithm).await?.value
        == rd_files::compute_checksum(second, algorithm).await?.value)
}

/// Whether a file is there. A path below something that is not a folder holds nothing either.
async fn exists(path: &Path) -> Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if absent(&error) => Ok(false),
        Err(error) => Err(error).with_context(|| format!("look for {}", path.display())),
    }
}

/// The answers that mean "nothing under that name".
fn absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

async fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}

/// Removes one file; one that is not there is fine.
async fn discard(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if absent(&error) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
#[path = "relocate_tests.rs"]
mod tests;

#[cfg(all(test, feature = "failpoints"))]
#[path = "relocate_crash_tests.rs"]
mod crash_tests;

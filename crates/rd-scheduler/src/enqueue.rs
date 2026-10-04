//! Package-level enqueue used by the LinkGrabber (one package row, many files).

use std::path::PathBuf;

use anyhow::Result;
use rd_core::{
    AccountId, CategoryId, DownloadFile, DownloadPackage, DownloadPriority, DownloadState,
    PackageId, ProxyProfileId,
};
use rd_db::{Database, NewDownload, NewPackage, PackageChange};
use url::Url;

use crate::SchedulerHandle;

/// Package attributes for `enqueue_package`.
#[derive(Clone, Debug)]
pub struct PackageSpec {
    pub name: String,
    /// Category directory; the package is stored in a folder named after it below this path.
    pub destination: PathBuf,
    pub category_id: Option<CategoryId>,
    pub priority: DownloadPriority,
    pub password: Option<String>,
    pub start_paused: bool,
    /// Explicit post-processing level copied from the LinkGrabber package (`None` inherits).
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    /// Post-processing script copied from the LinkGrabber package.
    pub script: Option<String>,
    /// What the enrichers found for the package as a whole.
    ///
    /// Passed in rather than written afterwards: a package that is already readable while its
    /// enrichment is still one write away is a package somebody can see without it.
    pub enrichment: Vec<rd_core::EnrichmentField>,
}

/// One file of a package.
#[derive(Clone, Debug)]
pub struct FileSpec {
    pub source: Url,
    pub file_name: String,
    pub size: Option<rd_core::ByteCount>,
    pub account_id: Option<AccountId>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    /// Auth profile for the file; defaults to matching one by scope.
    pub auth_profile: rd_core::AuthProfileSelection,
    /// Transport (`Http` by default; `Media` for extractor-driven files).
    pub kind: rd_core::DownloadKind,
    pub media: Option<rd_core::MediaSelection>,
    /// Stored login for FTP/SFTP files; `None` matches by host and port at transfer time.
    pub remote_credential_id: Option<rd_core::RemoteCredentialId>,
    /// Consented replay template of an intercepted browser request, if any.
    pub replay: Option<ReplaySpec>,
    /// The vaulted link fragment this file inherits from its candidate (RD-110-38).
    ///
    /// A reference and the candidate it came from, never the fragment: ownership moves to
    /// the download row in the transaction that writes it, so the LinkGrabber entry can be
    /// deleted afterwards without taking the queue's key with it.
    pub secret_fragment: Option<SecretFragmentSpec>,
    /// Key shared with the other links in this package that point at the same file.
    pub mirror_group: Option<String>,
    /// Whether this link waits for another member of its group instead of starting.
    pub skipped: bool,
    /// What the enrichers found for this file; see [`PackageSpec::enrichment`].
    pub enrichment: Vec<rd_core::EnrichmentField>,
    /// Every source of the file and the hashes its bytes must match (RD-150-03), written with
    /// the row. Its whole-file hash becomes the download's expected checksum.
    pub source_set: Option<Box<rd_core::SourceSet>>,
}

/// The template, its consent and the vaulted body, on their way from a candidate to a
/// download. Ownership of `body_ref` moves with it.
#[derive(Clone, Debug)]
pub struct ReplaySpec {
    pub request: rd_core::CapturedRequest,
    pub consent: rd_core::ReplayConsent,
    /// `vault://` reference of the encrypted body; the candidate's column is cleared in the
    /// same transaction that writes the template.
    pub body_ref: Option<String>,
    /// Candidate the body reference is taken from.
    pub candidate_id: Option<rd_core::CandidateId>,
}

/// A vaulted link fragment on its way from a candidate to the download it becomes.
#[derive(Clone, Debug)]
pub struct SecretFragmentSpec {
    /// `vault://` reference of the fragment.
    pub reference: String,
    /// Candidate the reference is taken from; its column is cleared in the same transaction.
    pub candidate_id: Option<rd_core::CandidateId>,
}

impl SchedulerHandle {
    /// Creates a package with all files in the given order; the package lands at the end of
    /// the queue so LinkGrabber order is preserved when several packages are enqueued.
    ///
    /// Runs in the caller's future, deliberately, and must keep doing so.
    ///
    /// Detaching it onto a task of its own would close a cancellation gap — an Axum handler
    /// dropped on client disconnect abandons the loop between two `create_download` calls
    /// without reaching the rollback — but it opens a worse one. The callers follow this with
    /// work that belongs to the same operation, above all `carry_enrichment`, and a task
    /// boundary *here* lets a reader observe the package after it is written and before that
    /// work lands. That was tried and reverted.
    ///
    /// The gap is closed one layer up instead: `rd_api_intake::collector_enqueue::enqueue_package`
    /// runs the whole enqueue-and-enrich operation on a task, so there is no point inside it
    /// at which a half-written package is observable. Anything added between this call and the
    /// enrichment belongs inside that boundary, not behind a second one.
    pub async fn enqueue_package(
        &self,
        spec: PackageSpec,
        files: Vec<FileSpec>,
    ) -> Result<(DownloadPackage, Vec<DownloadFile>)> {
        self.enqueue_package_with_torrents(spec, files, Vec::new())
            .await
    }

    /// [`Self::enqueue_package`], carrying the reviewed torrent state -- file tree and
    /// selection -- of the files whose source is named in `torrent_states`.
    ///
    /// Written before the row can start (audit 1.9.1, API-07). The LinkGrabber used to write
    /// the selection after the whole package was enqueued, and a dispatcher pass in between
    /// started the torrent with the default selection, which its runner then persisted over
    /// the reviewed one. A file with a state is therefore created paused, gets its state, and
    /// only then joins the queue -- unless the package was asked to start paused anyway.
    pub async fn enqueue_package_with_torrents(
        &self,
        spec: PackageSpec,
        files: Vec<FileSpec>,
        torrent_states: Vec<(Url, rd_core::TorrentJobState)>,
    ) -> Result<(DownloadPackage, Vec<DownloadFile>)> {
        anyhow::ensure!(!files.is_empty(), "package contains no files");
        write_package(&self.database, spec, files, torrent_states).await
    }
}

/// The body of `enqueue_package`. Every way out of this function runs the rollback first, so
/// the only remaining way to leave rows behind is the future being dropped mid-loop.
async fn write_package(
    database: &Database,
    spec: PackageSpec,
    files: Vec<FileSpec>,
    mut torrent_states: Vec<(Url, rd_core::TorrentJobState)>,
) -> Result<(DownloadPackage, Vec<DownloadFile>)> {
    let package_id = PackageId::new();
    let destination = rd_files::package_directory(&spec.destination, &spec.name);
    let package = database
        .create_package(NewPackage {
            id: package_id,
            name: spec.name,
            destination: destination.to_string_lossy().into_owned(),
            category_id: spec.category_id,
            priority: spec.priority,
            postprocess_level: spec.postprocess_level,
            script: spec.script,
            enrichment: spec.enrichment,
        })
        .await?;
    // The package row is written before its first file, and there is no way round that: a
    // download row needs a package to belong to. A stop in the window between the two is
    // therefore the one interruption this path cannot prevent, only survive — see
    // `crates/rd-core/recovery-matrix.md`.
    rd_core::failpoint!("scheduler.after_package_row", || anyhow::anyhow!(
        "crash point: the package row is written and no file is"
    ));
    if spec.password.is_some()
        && let Err(error) = database
            .update_packages(
                vec![package_id],
                PackageChange {
                    category: None,
                    priority: None,
                    name: None,
                    password: Some(spec.password),
                    postprocess_level: None,
                    script: None,
                },
            )
            .await
    {
        // The package row exists and has no files yet: the same empty-package case the first
        // `create_download` failure hits, so it takes the same route out.
        roll_back_partial_package(database, package_id, &[]).await;
        return Err(error.context("package password could not be stored"));
    }
    let mut created = Vec::with_capacity(files.len());
    for file in files {
        let torrent_state = torrent_states
            .iter()
            .position(|(source, _)| *source == file.source)
            .map(|index| torrent_states.swap_remove(index).1);
        let new_download = NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: file.source,
            file_name: rd_files::sanitize_file_name(&file.file_name),
            // A Metalink file states its size; the link itself was proposed without one.
            total_bytes: file.size.or_else(|| {
                file.source_set
                    .as_ref()
                    .and_then(|set| set.size)
                    .and_then(|size| rd_core::ByteCount::new(size).ok())
            }),
            expected_checksum: file
                .source_set
                .as_ref()
                .and_then(|set| set.checksum.clone()),
            account_id: file.account_id,
            proxy_profile_id: file.proxy_profile_id,
            auth_profile: file.auth_profile,
            initial_state: if file.skipped {
                // A mirror waits for the member that is downloading, whatever
                // the package's own start mode says.
                DownloadState::Skipped
            } else if spec.start_paused || torrent_state.is_some() {
                // A torrent waits for its reviewed selection; see
                // `SchedulerHandle::enqueue_package_with_torrents`.
                DownloadState::Paused
            } else {
                DownloadState::Queued
            },
            kind: file.kind,
            media: file.media,
            remote_credential_id: file.remote_credential_id,
            mirror_group: file.mirror_group,
            enrichment: file.enrichment,
            replay: file.replay.map(|replay| {
                Box::new(rd_db::NewReplayTemplate {
                    request: replay.request,
                    consent: replay.consent,
                    body_ref: replay.body_ref,
                    candidate_id: replay.candidate_id,
                })
            }),
            secret_fragment: file.secret_fragment.map(|fragment| {
                Box::new(rd_db::NewSecretFragment {
                    reference: fragment.reference,
                    candidate_id: fragment.candidate_id,
                })
            }),
        };
        let created_file = match file.source_set {
            Some(set) => {
                database
                    .create_download_with_sources(new_download, *set)
                    .await
            }
            None => database.create_download(new_download).await,
        };
        let (created_file, joins_queue) = match (created_file, torrent_state) {
            (Ok(download), Some(state)) => (
                store_torrent_state(database, download, state).await,
                !spec.start_paused,
            ),
            (created_file, _) => (created_file, false),
        };
        if joins_queue {
            // The row is paused and holds its reviewed selection; a stop here leaves it
            // exactly so, never queued with the default one (`crates/rd-core/recovery-matrix.md`).
            rd_core::failpoint!("scheduler.after_torrent_selection", || {
                anyhow::anyhow!(
                    "crash point: the torrent selection is written and the row is not queued"
                )
            });
        }
        let created_file = match created_file {
            Ok(download) if joins_queue && download.state == DownloadState::Paused => {
                join_queue(database, download).await
            }
            other => other,
        };
        match created_file {
            Ok(download) => created.push(download),
            // A package holding part of its file set is worse than no package at all:
            // nothing in the queue or the interface distinguishes it from a package that
            // is simply short, so it reads as complete, while the links that never made
            // it are gone with the LinkGrabber entry they came from.
            Err(error) => {
                roll_back_partial_package(database, package_id, &created).await;
                return Err(error.context("package file could not be created"));
            }
        }
    }
    Ok((package, created))
}

/// Writes a torrent row's reviewed state while the row is still paused.
///
/// On a failure the row is removed again, so the caller's rollback sees only the rows that
/// were completely written.
async fn store_torrent_state(
    database: &Database,
    download: DownloadFile,
    state: rd_core::TorrentJobState,
) -> Result<DownloadFile> {
    match database
        .set_download_torrent_state(download.id, state)
        .await
    {
        Ok(()) => Ok(download),
        Err(error) => {
            remove_unfinished_row(database, &download).await;
            Err(error)
        }
    }
}

/// Lets a torrent row whose selection is stored join the queue; on a failure the row is
/// removed again, as in [`store_torrent_state`].
///
/// Only a row still paused since it was written moves: a pause or resume that came in
/// between is not overwritten from the snapshot, and the move announces no transition of its
/// own, as a row created queued announces none (re-audit 1.9.1, RA-TR-08, RA-API-06).
async fn join_queue(database: &Database, download: DownloadFile) -> Result<DownloadFile> {
    match database.join_queue(download.id, download.updated_at).await {
        Ok(queued) => Ok(queued),
        Err(error) => {
            remove_unfinished_row(database, &download).await;
            Err(error)
        }
    }
}

async fn remove_unfinished_row(database: &Database, download: &DownloadFile) {
    if let Err(error) = database.delete_download(download.id).await {
        tracing::warn!(
            %error,
            download = %download.id,
            "a torrent row that never joined the queue could not be removed"
        );
    }
}

/// Undoes a half-written package. Every file this call created is removed, and the package
/// row goes with the last of them — `delete_download` drops a package that has no files left.
///
/// The package that has no file at all is the case that used to be merely logged: there was no
/// queue entry to hang the deletion on and rd-db exposed no package delete of its own. It does
/// now, and `delete_empty_package` is a no-op on a package that still has files, so this can
/// end with it unconditionally rather than having to work out which of the deletions above
/// already took the package along.
async fn roll_back_partial_package(
    database: &Database,
    package_id: PackageId,
    created: &[DownloadFile],
) {
    for download in created {
        if let Err(error) = database.delete_download(download.id).await {
            tracing::warn!(
                %error,
                download = %download.id,
                package = %package_id,
                "a half-written package could not be rolled back"
            );
            return;
        }
    }
    if let Err(error) = database.delete_empty_package(package_id).await {
        tracing::warn!(
            %error,
            package = %package_id,
            "an empty package row could not be removed"
        );
    }
}

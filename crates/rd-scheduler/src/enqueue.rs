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
    /// The gap is closed one layer up instead: `rd_api::collector_enqueue::enqueue_package`
    /// runs the whole enqueue-and-enrich operation on a task, so there is no point inside it
    /// at which a half-written package is observable. Anything added between this call and the
    /// enrichment belongs inside that boundary, not behind a second one.
    pub async fn enqueue_package(
        &self,
        spec: PackageSpec,
        files: Vec<FileSpec>,
    ) -> Result<(DownloadPackage, Vec<DownloadFile>)> {
        anyhow::ensure!(!files.is_empty(), "package contains no files");
        write_package(&self.database, spec, files).await
    }
}

/// The body of `enqueue_package`. Every way out of this function runs the rollback first, so
/// the only remaining way to leave rows behind is the future being dropped mid-loop.
async fn write_package(
    database: &Database,
    spec: PackageSpec,
    files: Vec<FileSpec>,
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
    // `docs/recovery-matrix.md`.
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
        let created_file = database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id,
                source: file.source,
                file_name: rd_files::sanitize_file_name(&file.file_name),
                total_bytes: file.size,
                expected_checksum: None,
                account_id: file.account_id,
                proxy_profile_id: file.proxy_profile_id,
                auth_profile: file.auth_profile,
                initial_state: if file.skipped {
                    // A mirror waits for the member that is downloading, whatever
                    // the package's own start mode says.
                    DownloadState::Skipped
                } else if spec.start_paused {
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
            })
            .await;
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

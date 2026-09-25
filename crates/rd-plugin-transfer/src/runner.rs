//! One queue row, one plugin transfer attempt.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind, StorageRootId};
use rd_files::{PartFile, StorageRoot};
use rd_plugin_host::{TransferBackend, TransferOutcome, TransferState, TransferTarget};
use rd_scheduler::{ExternalRunner, RunLimits, RunOutcome};
use tokio_util::sync::CancellationToken;

/// One database write per megabyte, as the native runners do: the resume-relevant state is
/// the file on disk, the row only feeds the progress bar.
const PROGRESS_INTERVAL_BYTES: u64 = 1024 * 1024;

/// Carries queue rows whose protocol comes from an installed backend.
pub struct PluginTransferRunner {
    backends: crate::TransferBackends,
    database: rd_db::Database,
    tls: Arc<rustls::ClientConfig>,
}

impl PluginTransferRunner {
    #[must_use]
    pub fn new(
        backends: crate::TransferBackends,
        database: rd_db::Database,
        custom_ca_pem: Vec<Vec<u8>>,
    ) -> Self {
        // A broken custom CA is refused rather than silently replaced by the platform roots;
        // if it cannot be built here the transfers fail at the handshake with a clear error
        // instead of trusting a different set of issuers.
        let tls = rd_http::tls_client_config(&custom_ca_pem)
            .or_else(|error| {
                tracing::warn!(error = %error, "custom CA unusable for plugin transfers");
                rd_http::tls_client_config(&[])
            })
            .map(Arc::new);
        Self {
            backends,
            database,
            tls: tls.unwrap_or_else(|_| {
                unreachable!("the platform trust store is required for any TLS at all")
            }),
        }
    }

    /// The backend this row must run on: the pinned one while a transfer is in flight,
    /// otherwise the newest that claims the scheme.
    async fn backend_for(
        &self,
        file: &DownloadFile,
        scheme: &str,
    ) -> Result<(Arc<TransferBackend>, Option<Vec<u8>>), Failure> {
        let pinned = self
            .database
            .plugin_transfer(file.id)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(error = %error, "could not read the plugin transfer checkpoint");
                None
            });
        if let Some(state) = pinned {
            let Some(backend) = self
                .backends
                .pinned(&state.plugin_id, &state.plugin_version)
            else {
                // The half-written file belongs to a format only that build could read, so
                // continuing with another version would corrupt it silently.
                return Err(Failure::coded(
                    FailureKind::Unsupported,
                    "plugin.pinned_version_missing",
                    "The transfer backend version this download started on is not installed",
                ));
            };
            return Ok((Arc::clone(backend), state.checkpoint));
        }
        let backend = self.backends.for_scheme(scheme).ok_or_else(|| {
            Failure::coded(
                FailureKind::Unsupported,
                "plugin.no_backend_for_scheme",
                "No installed plugin handles this link's protocol",
            )
        })?;
        Ok((Arc::clone(backend), None))
    }
}

#[async_trait]
impl ExternalRunner for PluginTransferRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Plugin
    }

    fn slot_capacity(&self) -> usize {
        // The strictest backend decides, because one semaphore covers the whole kind.
        self.backends.concurrency_floor().unwrap_or(1)
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: RunLimits,
    ) -> Result<RunOutcome> {
        let url = url::Url::parse(file.source.as_str())
            .with_context(|| format!("parse plugin transfer URL for {}", file.id))?;
        let (backend, checkpoint) = match self.backend_for(file, url.scheme()).await {
            Ok(found) => found,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };

        if package.destination.is_empty() {
            anyhow::bail!("plugin transfer package has no destination directory");
        }
        let root = StorageRoot::create(
            StorageRootId::new(),
            "download destination".to_owned(),
            PathBuf::from(&package.destination),
        )
        .await?;
        tokio::fs::create_dir_all(root.path()).await?;
        let part_path = rd_files::part_path(&root, file.id).await?;

        let credential_ref = file.remote_credential_id.map(|id| id.to_string());
        // The probe writes nothing, but it runs against a real store so a backend cannot
        // tell the two calls apart and keep state between them.
        let probe_state = self.state(
            &backend,
            file.id,
            TransferTarget {
                part: PartFile::open(part_path.clone(), None).await?,
                committed: 0,
                total: None,
            },
            &cancellation,
            &limits,
        );
        let remote = match backend
            .probe(probe_state, file.source.to_string(), credential_ref.clone())
            .await
        {
            Ok(remote) => remote,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };

        // The staging file and its length come from `rd-files`, not from
        // `rd_transfer_file::Staging`, which owns the same two things for FTP and SFTP. That
        // type is built around a host-side loop that reads an `AsyncRead` into a file it
        // opened itself and syncs it before the rename; a plugin transfer has no source in the
        // host at all. The guest writes through the `PartFile` it was handed, the throttle and
        // the progress write live in `TransferState`, and the staging file has to exist before
        // the probe can run — so before any size is known, where `Staging` wants an exact one.
        let committed = rd_files::existing_bytes(&part_path).await;
        if let Some(failure) = self.validate_resume(file, &remote, committed).await {
            return Ok(RunOutcome::Failed(failure));
        }
        // A backend that cannot continue starts over rather than writing new bytes on top of
        // old ones, which is the one way a resume can corrupt without ever erroring.
        let committed = if remote.resumable { committed } else { 0 };

        let part = PartFile::open(part_path.clone(), remote.size).await?;
        let state = self.state(
            &backend,
            file.id,
            TransferTarget {
                // The guest gets a clone and the runner keeps the original, because the
                // promotion below is the host's decision, not the backend's. The clone shares
                // one file handle, so it has to be gone before the rename: it lives only in
                // `state`, which is moved into `run` and dropped with the guest's store when
                // that call returns. `finalize` checks that rather than trusting it.
                part: part.clone(),
                committed,
                total: remote.size,
            },
            &cancellation,
            &limits,
        );
        let job = rd_plugin_host::TransferJob {
            url: file.source.to_string(),
            credential_ref,
            checkpoint,
        };
        let manifest = backend.manifest();
        let plugin_id = manifest.id.to_string();
        let version = manifest.version.clone();

        match backend.run(state, job).await {
            Ok(TransferOutcome::Stopped {
                committed,
                checkpoint,
            }) => {
                self.persist(file, &plugin_id, &version, Some(checkpoint))
                    .await;
                let _ = self
                    .database
                    .set_download_progress(file.id, committed, remote.size)
                    .await;
                Ok(RunOutcome::Stopped)
            }
            Ok(TransferOutcome::Complete { committed, .. }) => {
                // The host decides what "complete" means. A backend reporting success on a
                // short file is a failed attempt, not a promoted stub.
                if let Some(size) = remote.size
                    && committed < size
                {
                    self.persist(file, &plugin_id, &version, None).await;
                    return Ok(RunOutcome::Failed(Failure::coded(
                        FailureKind::Transient {
                            retry_after_seconds: None,
                        },
                        "plugin.transfer_incomplete",
                        "The transfer ended before the whole file arrived",
                    )));
                }
                let name = rd_files::sanitize_file_name(&file.file_name);
                let final_path = root.resolve(std::path::Path::new(&name))?;
                if let Some(parent) = final_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                // Fails rather than renaming if the guest's clone somehow outlived `run`.
                // Promoting behind an open handle is a sharing violation on Windows and a
                // silent write into an already published file everywhere else.
                part.finalize(&final_path).await?;
                let _ = self.database.clear_plugin_transfer(file.id).await;
                let _ = self
                    .database
                    .set_download_progress(file.id, committed, remote.size)
                    .await;
                Ok(RunOutcome::Completed { final_name: name })
            }
            Err(failure) => {
                self.persist(file, &plugin_id, &version, None).await;
                Ok(RunOutcome::Failed(failure))
            }
        }
    }
}

impl PluginTransferRunner {
    fn state(
        &self,
        backend: &TransferBackend,
        file: rd_core::DownloadId,
        target: TransferTarget,
        cancellation: &CancellationToken,
        limits: &RunLimits,
    ) -> TransferState {
        let committed = target.committed;
        let database = self.database.clone();
        let reported = Arc::new(std::sync::atomic::AtomicU64::new(committed));
        // One row update per megabyte and never on the guest's thread: the resume-relevant
        // state is the file on disk, so a slow write here must not pace the transfer.
        let progress = Arc::new(move |committed: u64, total: Option<u64>| {
            let previous = reported.load(std::sync::atomic::Ordering::Relaxed);
            if committed.saturating_sub(previous) < PROGRESS_INTERVAL_BYTES {
                return;
            }
            reported.store(committed, std::sync::atomic::Ordering::Relaxed);
            let database = database.clone();
            tokio::spawn(async move {
                let _ = database.set_download_progress(file, committed, total).await;
            });
        });
        let state = TransferState::new(
            target,
            cancellation.clone(),
            limits.bandwidth.clone(),
            backend.manifest().capabilities.net_stream.clone(),
            Arc::clone(&self.tls),
            progress,
        );
        if self.backends.allows_local_targets() {
            state.allowing_local_targets()
        } else {
            state
        }
    }

    async fn validate_resume(
        &self,
        file: &DownloadFile,
        remote: &rd_plugin_host::RemoteFile,
        committed: u64,
    ) -> Option<Failure> {
        if committed == 0 {
            let _ = self
                .database
                .prepare_transfer(
                    file.id,
                    remote.size,
                    None,
                    remote.last_modified.clone(),
                    Vec::new(),
                )
                .await;
            return None;
        }
        let stored = self.database.load_transfer(file.id).await.ok()?;
        // Both validators are compared verbatim: a server that moved either has a different
        // file behind the same URL, and continuing would splice two of them together.
        let changed = stored.total_bytes != remote.size
            || (stored.last_modified.is_some() && stored.last_modified != remote.last_modified)
            || remote.size.is_some_and(|size| committed > size);
        changed.then(|| {
            Failure::coded(
                FailureKind::Permanent,
                "plugin.file_changed",
                "The remote file changed since this download started",
            )
        })
    }

    async fn persist(
        &self,
        file: &DownloadFile,
        plugin_id: &str,
        version: &str,
        checkpoint: Option<Vec<u8>>,
    ) {
        if let Err(error) = self
            .database
            .save_plugin_transfer(
                file.id,
                plugin_id.to_owned(),
                version.to_owned(),
                checkpoint,
            )
            .await
        {
            tracing::warn!(error = %error, "could not persist the plugin transfer checkpoint");
        }
    }
}

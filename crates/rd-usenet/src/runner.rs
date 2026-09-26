//! Usenet transport as a scheduler runner: one NZB file per queue entry.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use rd_core::{
    DownloadFile, DownloadKind, DownloadPackage, DownloadState, Failure, FailureKind, StorageRootId,
};
use rd_db::Database;
use rd_files::StorageRoot;
use rd_http::SharedNetworkDefaults;
use rd_scheduler::{ExternalRunner, RunOutcome};
use rd_secrets::SecretStore;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool,
    parallel::{FileLoad, MAX_PARALLEL_FILES},
    worker::{FileOutcome, download_file_counted, recovered_file_path},
};

/// Limits applied per NZB file.
///
/// How many files run at once is not in here: it is the operator's setting, which reaches
/// the runner through the scheduler with every dispatch pass, and automatic by default
/// (RD-130-22, see `src/parallel.rs`).
#[derive(Clone, Debug)]
pub struct UsenetRunnerConfig {
    pub max_file_bytes: u64,
}

impl Default for UsenetRunnerConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: 128 * 1024 * 1024 * 1024,
        }
    }
}

/// Downloads NZB files through the configured NNTP servers.
pub struct UsenetRunner {
    database: Database,
    secrets: SecretStore,
    config: UsenetRunnerConfig,
    /// The proxy and custom-CA defaults the whole service shares, so a news server is trusted
    /// by the same rule as an HTTP host and an FTPS server. Default until
    /// [`Self::with_network_defaults`] hands over the scheduler's, which is the platform store
    /// alone - the behaviour of an installation without a custom CA.
    network: SharedNetworkDefaults,
    /// The pool, kept between files (RD-108-26). One NZB used to build its own, so every file
    /// change cost a TCP connection, a TLS handshake and an `AUTHINFO` per connection - ten
    /// of each on the instance this was measured on, at every one of a release's fifty files.
    pool: Mutex<Option<CachedPool>>,
    /// What the running files still have to fetch, which is what automatic mode sizes the
    /// number of files by (RD-130-22).
    load: Arc<FileLoad>,
}

/// A pool and what it was built from; it is replaced when either changes.
struct CachedPool {
    fingerprint: String,
    cap: Option<usize>,
    pool: NntpPool,
}

impl UsenetRunner {
    #[must_use]
    pub fn new(database: Database, secrets: SecretStore, config: UsenetRunnerConfig) -> Self {
        Self {
            database,
            secrets,
            config,
            network: SharedNetworkDefaults::default(),
            pool: Mutex::new(None),
            load: Arc::new(FileLoad::default()),
        }
    }

    /// Connects this runner to the shared network defaults, which is where the operator's
    /// custom CA lives.
    #[must_use]
    pub fn with_network_defaults(mut self, network: SharedNetworkDefaults) -> Self {
        self.network = network;
        self
    }

    /// The pool for the current settings, built only if there is not one already.
    pub(crate) async fn pool(&self, cap: Option<usize>) -> Result<NntpPool> {
        let (custom_ca_pem, tls_revision) = {
            let defaults = self.network.read().await;
            (defaults.custom_ca_pem.clone(), defaults.tls_revision)
        };
        // The TLS revision belongs in the key for the same reason the server records do: a pool
        // that outlives a single file would otherwise keep handing out connections built on the
        // trust roots the operator has just replaced.
        let fingerprint = format!(
            "{}|tls{tls_revision}",
            crate::connection_fingerprint(&self.database).await?
        );
        let mut cached = self.pool.lock().await;
        if let Some(current) = cached.as_ref()
            && current.fingerprint == fingerprint
            && current.cap == cap
        {
            return Ok(current.pool.clone());
        }
        let servers =
            crate::enabled_server_configs(&self.database, &self.secrets, &custom_ca_pem).await?;
        let pool = NntpPool::with_connection_cap(servers, cap)?;
        *cached = Some(CachedPool {
            fingerprint,
            cap,
            pool: pool.clone(),
        });
        Ok(pool)
    }
}

#[async_trait]
impl ExternalRunner for UsenetRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Usenet
    }

    /// The most files the runner ever works on at once; how many it takes right now is
    /// [`Self::dispatch_capacity`].
    fn slot_capacity(&self) -> usize {
        MAX_PARALLEL_FILES
    }

    fn dispatch_capacity(&self, requested_files: usize) -> usize {
        self.load.capacity(requested_files)
    }

    /// Every file goes through the one connection pool, so together they are one transfer:
    /// three running NZB files are the same ten connections as one (RD-130-22). Counting
    /// each of them against `max_active_files` would let the global limit decide how well
    /// the connections are used, which is the pool's business.
    fn shares_one_global_slot(&self) -> bool {
        true
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: rd_scheduler::RunLimits,
    ) -> Result<RunOutcome> {
        let import_id = package
            .nzb_import_id
            .context("usenet package without NZB import")?;
        let nzb_file_id = file
            .nzb_file_id
            .context("usenet download without NZB file")?;
        let files = self.database.list_nzb_files(import_id).await?;
        let Some(nzb_file) = files.iter().find(|item| item.id == nzb_file_id) else {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                "usenet.nzb_missing",
                "NZB file no longer exists",
            )));
        };
        // On the books from here to the end of the attempt, whichever way it ends.
        let open = self.load.start(
            nzb_file
                .segments
                .iter()
                .filter(|segment| segment.state != rd_core::NzbSegmentState::Completed)
                .count(),
        );
        if nzb_file.total_bytes.get() > self.config.max_file_bytes {
            return Ok(RunOutcome::Failed(
                Failure::coded(
                    FailureKind::Permanent,
                    "usenet.nzb_too_large",
                    "NZB file exceeds the configured size limit",
                )
                .with_param("limit", self.config.max_file_bytes),
            ));
        }
        if package.destination.is_empty() {
            bail!("usenet package has no destination directory");
        }
        let root = StorageRoot::create(
            StorageRootId::new(),
            "download destination".to_owned(),
            PathBuf::from(&package.destination),
        )
        .await?;
        let destination = root.path().to_path_buf();
        tokio::fs::create_dir_all(&destination).await?;
        if let Some(path) = recovered_file_path(nzb_file, &destination).await? {
            let final_name = file_name_of(&path)?;
            self.settle(file, &path, &final_name).await?;
            return Ok(RunOutcome::Completed { final_name });
        }
        let staging = root.resolve(std::path::Path::new(&format!(".rdownloader-{import_id}")))?;
        tokio::fs::create_dir_all(&staging).await?;
        // `0` is "as many as the servers allow" (RD-108-25); anything else caps one file.
        let cap = (limits.max_parallel_requests > 0).then_some(limits.max_parallel_requests);
        let pool = self.pool(cap).await?;
        self.load.set_window(pool.max_parallel_requests());
        let outcome = download_file_counted(
            &self.database,
            &pool,
            &cancellation,
            nzb_file,
            &staging,
            &destination,
            &limits,
            &open,
        )
        .await;
        drop(open);
        let (waited, batches, confirmed) = self.load.writer_totals();
        tracing::debug!(
            download_id = %file.id,
            writer_wait_ms = waited / 1_000_000,
            batches,
            confirmed,
            "usenet assembly checkpoints since start"
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            // Coded failures describe the file (not the runner) and belong in the queue.
            Err(error) => {
                return match error.downcast::<Failure>() {
                    Ok(failure) => Ok(RunOutcome::Failed(failure)),
                    Err(error) => Err(error),
                };
            }
        };
        match outcome {
            FileOutcome::Cancelled => Ok(RunOutcome::Stopped),
            FileOutcome::Completed { path, missing } => {
                let final_name = file_name_of(&path)?;
                self.settle(file, &path, &final_name).await?;
                let _ = tokio::fs::remove_dir(&staging).await;
                if missing > 0 {
                    // RD-108-24: the verdict is the set's to give, not this file's. Asking
                    // here answers on whatever the package happens to have shown of itself
                    // by now, and a fully obfuscated set has shown nothing until its PAR2
                    // files are assembled - which is how an NZB with seven of them came to
                    // be told it contained none. The row waits, carrying the reason, and
                    // `settle_par2_verdicts` decides it when nothing is on its way any more.
                    // A package that has nothing left running is decided by that same call,
                    // during the transition below, so a set without PAR2 fails as promptly
                    // as it always did.
                    tracing::info!(
                        download_id = %file.id,
                        missing,
                        "segments missing; the verdict waits for the rest of the set"
                    );
                    self.database.defer_par2_verdict(file.id, missing).await?;
                    return Ok(RunOutcome::Detached {
                        state: DownloadState::Verifying,
                    });
                }
                Ok(RunOutcome::Completed { final_name })
            }
        }
    }
}

impl UsenetRunner {
    /// The name is known now, so the PAR2 question is asked again (RD-108-23).
    ///
    /// Every decision taken at enqueue time rested on the subject line; an obfuscated post
    /// leaves that useless. The file on disk settles it: its name, and its header - SABnzbd's
    /// `handle_par2` recognises the file by content, and so does this.
    async fn settle(&self, file: &DownloadFile, path: &Path, final_name: &str) -> Result<()> {
        let on_disk = path.to_path_buf();
        let content_is_par2 =
            tokio::task::spawn_blocking(move || rd_files::has_par2_magic(&on_disk)).await?;
        let postponed = self
            .database
            .settle_nzb_recovery(file.id, final_name.to_owned(), content_is_par2)
            .await?;
        if postponed > 0 {
            tracing::info!(
                download_id = %file.id,
                index = final_name,
                postponed,
                "main PAR2 index assembled; the set's waiting volumes are postponed"
            );
        }
        Ok(())
    }
}

fn file_name_of(path: &std::path::Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .context("final file name is not UTF-8")
}

//! Usenet transport as a scheduler runner: one NZB file per queue entry.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use rd_core::{
    DownloadFile, DownloadKind, DownloadPackage, DownloadState, Failure, FailureKind, NzbImportId,
    StorageRootId,
};
use rd_db::Database;
use rd_files::StorageRoot;
use rd_http::SharedNetworkDefaults;
use rd_scheduler::{ExternalRunner, RunOutcome};
use rd_secrets::SecretStore;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, UsenetTraffic,
    hopeless::{Ending, Verdicts},
    parallel::{FileLoad, MAX_PARALLEL_FILES},
    worker::{FileOutcome, download_file_counted, recovered_file_path},
};

/// How long a download waits before it asks again when every enabled server is paused by its
/// quota (RD-1100-05). A wait for a limit, not an attempt: a quota is raised or reset by a
/// person or on its reset day, and the queue should notice within minutes, not hours.
const QUOTA_RETRY_SECONDS: u64 = 15 * 60;

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
    /// The sets given up as beyond repair while files of them run (RD-1100-02).
    verdicts: Verdicts,
    /// The bytes each server delivered since the last flush (RD-1100-05).
    traffic: UsenetTraffic,
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
            verdicts: Verdicts::default(),
            traffic: UsenetTraffic::default(),
        }
    }

    /// Counts the bytes each server delivers into `traffic`, which the service flushes
    /// (RD-1100-05). Without it the runner counts into a meter nobody writes.
    #[must_use]
    pub fn with_traffic(mut self, traffic: UsenetTraffic) -> Self {
        self.traffic = traffic;
        self
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
        let ordered =
            crate::servers_by_quota(&self.database, &self.secrets, &custom_ca_pem).await?;
        if ordered.servers.is_empty() && ordered.paused > 0 {
            return Err(Failure::coded(
                FailureKind::RateLimited {
                    retry_after_seconds: Some(QUOTA_RETRY_SECONDS),
                },
                "usenet.quota_reached",
                "Every enabled Usenet server is paused because its quota is used up",
            )
            .with_param("servers", ordered.paused)
            .into());
        }
        let servers = ordered
            .servers
            .into_iter()
            .map(|(id, config)| (config, Some(self.traffic.counter(id))))
            .collect();
        let pool = NntpPool::metered(servers, cap)?;
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

    /// Article ranges already in the part file are proven by their checksums before they are
    /// kept, an already renamed output file is recognised after a restart, and yEnc checksums
    /// verify every article; the names come from the NZB, so they keep their own rule.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: true,
            recheck_partial: true,
            adopt_completed: true,
            verify_completed: true,
            applies_collision_policy: false,
        }
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
        // A set given up while this file waited for its turn takes no new attempt, and one
        // given up while it runs stops it (RD-1100-02).
        let enrolment = self.verdicts.enrol(import_id);
        if let Some(failure) = self.verdicts.verdict(import_id) {
            return Ok(RunOutcome::Failed(failure));
        }
        let staging = root.resolve(std::path::Path::new(&format!(".rdownloader-{import_id}")))?;
        tokio::fs::create_dir_all(&staging).await?;
        // `0` is "as many as the servers allow" (RD-108-25); anything else caps one file.
        let cap = (limits.max_parallel_requests > 0).then_some(limits.max_parallel_requests);
        let pool = match self.pool(cap).await {
            Ok(pool) => pool,
            // Every server paused by its quota is a wait, not a broken runner.
            Err(error) => {
                return match error.downcast::<Failure>() {
                    Ok(failure) => Ok(RunOutcome::Failed(failure)),
                    Err(error) => Err(error),
                };
            }
        };
        self.load.set_window(pool.max_parallel_requests());
        // The attempt stops for the queue's reason or for the set's, whichever comes first.
        let attempt = enrolment.abort().child_token();
        let link = {
            let scheduler = cancellation.clone();
            let attempt = attempt.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = scheduler.cancelled() => attempt.cancel(),
                    () = attempt.cancelled() => {}
                }
            })
        };
        let outcome = download_file_counted(
            &self.database,
            &pool,
            &attempt,
            nzb_file,
            &staging,
            &destination,
            &limits,
            &open,
        )
        .await;
        link.abort();
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
                    // A file no server had a single article of is the set's loss too.
                    Ok(failure) if lost_everything(&failure) => Ok(RunOutcome::Failed(
                        self.abandon_if_hopeless(file, package, import_id, Ending::Lost)
                            .await?
                            .unwrap_or(failure),
                    )),
                    Ok(failure) => Ok(RunOutcome::Failed(failure)),
                    Err(error) => Err(error),
                };
            }
        };
        match outcome {
            // Stopped because its set was given up rather than by the queue.
            FileOutcome::Cancelled if !cancellation.is_cancelled() => {
                Ok(match self.verdicts.verdict(import_id) {
                    Some(failure) => RunOutcome::Failed(failure),
                    None => RunOutcome::Stopped,
                })
            }
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
                    // Unless the holes already show the set cannot be repaired (RD-1100-02):
                    // then the rest of it is not fetched to find that out at the end.
                    if let Some(failure) = self
                        .abandon_if_hopeless(file, package, import_id, Ending::Holes)
                        .await?
                    {
                        return Ok(RunOutcome::Failed(failure));
                    }
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
    /// Gives the set up when `file`, ending as `ending`, leaves it beyond repair (RD-1100-02),
    /// and answers with the failure the file then ends with.
    async fn abandon_if_hopeless(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        import_id: NzbImportId,
        ending: Ending,
    ) -> Result<Option<Failure>> {
        // Given up already by a sibling: one verdict, one write, one notification.
        if let Some(failure) = self.verdicts.verdict(import_id) {
            return Ok(Some(failure));
        }
        let Some(failure) =
            crate::hopeless::judge(&self.database, package.id, import_id, file.id, ending).await?
        else {
            return Ok(None);
        };
        crate::hopeless::give_up(
            &self.database,
            &self.verdicts,
            package.id,
            import_id,
            &failure,
        )
        .await?;
        Ok(Some(failure))
    }

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

/// Whether a coded failure says that no server had a single article of the file.
fn lost_everything(failure: &Failure) -> bool {
    matches!(
        failure.code.as_deref(),
        Some("usenet.all_segments_missing" | "usenet.recovery_unavailable")
    )
}

fn file_name_of(path: &std::path::Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .context("final file name is not UTF-8")
}

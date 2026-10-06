//! Usenet transport as a scheduler runner: one NZB file per queue entry.

use std::{
    ops::ControlFlow,
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

#[path = "runner_pool.rs"]
mod cached_pool;

use cached_pool::CachedPool;

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
        let (root, destination) = destination_root(package).await?;
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
        let (staging, pool) = match self.staging_and_pool(&root, import_id, &limits).await? {
            ControlFlow::Continue(ready) => ready,
            ControlFlow::Break(outcome) => return Ok(outcome),
        };
        self.load.set_window(pool.max_parallel_requests());
        // The attempt stops for the queue's reason or for the set's, whichever comes first.
        let attempt = enrolment.abort().child_token();
        let link = link_cancellation(&cancellation, &attempt);
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
        self.log_writer_totals(file);
        self.conclude(outcome, file, package, import_id, &cancellation, &staging)
            .await
    }
}

impl UsenetRunner {
    /// The set's staging directory and the pool the attempt fetches through; `Break` with the
    /// failure the file waits on when every server is paused by its quota.
    async fn staging_and_pool(
        &self,
        root: &StorageRoot,
        import_id: NzbImportId,
        limits: &rd_scheduler::RunLimits,
    ) -> Result<ControlFlow<RunOutcome, (PathBuf, NntpPool)>> {
        let staging = root.resolve(std::path::Path::new(&format!(".rdownloader-{import_id}")))?;
        tokio::fs::create_dir_all(&staging).await?;
        // `0` is "as many as the servers allow" (RD-108-25); anything else caps one file.
        let cap = (limits.max_parallel_requests > 0).then_some(limits.max_parallel_requests);
        let pool = match self.pool(cap).await {
            Ok(pool) => pool,
            // Every server paused by its quota is a wait, not a broken runner.
            Err(error) => {
                return match error.downcast::<Failure>() {
                    Ok(failure) => Ok(ControlFlow::Break(RunOutcome::Failed(failure))),
                    Err(error) => Err(error),
                };
            }
        };
        Ok(ControlFlow::Continue((staging, pool)))
    }

    /// How the assembly checkpoints have waited on the writer since start.
    fn log_writer_totals(&self, file: &DownloadFile) {
        let (waited, batches, confirmed) = self.load.writer_totals();
        tracing::debug!(
            download_id = %file.id,
            writer_wait_ms = waited / 1_000_000,
            batches,
            confirmed,
            "usenet assembly checkpoints since start"
        );
    }

    /// What the end of the file's attempt means for its queue entry.
    async fn conclude(
        &self,
        outcome: Result<FileOutcome>,
        file: &DownloadFile,
        package: &DownloadPackage,
        import_id: NzbImportId,
        cancellation: &CancellationToken,
        staging: &Path,
    ) -> Result<RunOutcome> {
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
                self.completed(file, package, import_id, staging, &path, missing)
                    .await
            }
        }
    }

    /// A file assembled at `path`: settled under its name, or, with articles `missing`, left
    /// to the set's PAR2 verdict.
    async fn completed(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        import_id: NzbImportId,
        staging: &Path,
        path: &Path,
        missing: usize,
    ) -> Result<RunOutcome> {
        let final_name = file_name_of(path)?;
        self.settle(file, path, &final_name).await?;
        let _ = tokio::fs::remove_dir(staging).await;
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

/// The package's destination as a storage root, created if it does not exist yet.
async fn destination_root(package: &DownloadPackage) -> Result<(StorageRoot, PathBuf)> {
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
    Ok((root, destination))
}

/// Cancels `attempt` when the queue cancels the run; ends by itself once the attempt is
/// cancelled for the set's reason.
fn link_cancellation(
    scheduler: &CancellationToken,
    attempt: &CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let scheduler = scheduler.clone();
    let attempt = attempt.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = scheduler.cancelled() => attempt.cancel(),
            () = attempt.cancelled() => {}
        }
    })
}

fn file_name_of(path: &std::path::Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .context("final file name is not UTF-8")
}

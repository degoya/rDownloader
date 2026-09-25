//! Package post-processing service: PAR2 repair, archive extraction, cleanup and user
//! scripts for finished downloads (SABnzbd-style levels per package/category).

mod cleanup_job;
mod package_job;
mod par2_job;
mod par2_refill;
mod pipeline;
mod plugin_step;
mod rar_test_job;
mod rclone_job;
mod remux_job;
mod script_job;
mod settings;
mod sfv_job;
mod steps;
mod storage_upload;
#[cfg(test)]
mod tests;
mod unpack_job;

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use rd_core::{DownloadState, EventKind, PackageId, PostprocessHold, PostprocessState};
use rd_db::Database;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

pub use plugin_step::{PluginStepJob, PluginStepOutcome, PluginStepRunner};
pub use settings::load_postprocess_settings;
pub use storage_upload::{StorageUpload, StorageUploader, UploadProgress, UploadReport};

/// Static configuration of the post-processing service.
#[derive(Clone, Debug)]
pub struct ExtractionConfig {
    /// `passwords.txt` used when the settings do not name a list.
    pub default_passwords_file: PathBuf,
    pub rar_timeout: Duration,
    /// Scripts directory used when the settings do not name one.
    pub default_scripts_directory: PathBuf,
    /// Shared with the scheduler so downloads can pause while post-processing runs.
    pub hold: PostprocessHold,
    /// Raised by the power service while quiet hours defer the resource-intensive steps
    /// (RD-050-13). A running job finishes; only the next one waits.
    pub quiet_hold: PostprocessHold,
}

/// Why a package extraction was requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionTrigger {
    /// Package became complete; the effective post-processing level decides what runs.
    Auto,
    /// User action; forces at least `Unpack` and re-runs completed sets.
    Manual,
    /// User action after a failed verification: like `Manual`, and the failure does not
    /// block the unpack for this one run (RD-104-04).
    ///
    /// The one-off counterpart to `safe_postproc`. Somebody looking at intact RAR volumes
    /// beside a broken recovery set wants exactly this once, and not a setting they then
    /// have to remember to put back.
    Force,
}

impl ExtractionTrigger {
    /// Whether the run was asked for by a person rather than by the queue.
    #[must_use]
    pub const fn is_manual(self) -> bool {
        matches!(self, Self::Manual | Self::Force)
    }
}

struct Job {
    package_id: PackageId,
    trigger: ExtractionTrigger,
}

struct Inner {
    database: Database,
    config: ExtractionConfig,
    hold: PostprocessHold,
    jobs: mpsc::Sender<Job>,
    in_flight: Mutex<HashSet<PackageId>>,
    shutdown: CancellationToken,
    /// The installed post-processing plugins, or `None` when none are loaded. Injected so
    /// this crate keeps its pipeline testable without a WebAssembly runtime.
    plugin_steps: Option<Arc<dyn plugin_step::PluginStepRunner>>,
    /// The installed upload destinations, injected for the same reason.
    storage: Option<Arc<dyn storage_upload::StorageUploader>>,
}

impl Inner {
    /// The stored login for a destination address, matched by host and port.
    ///
    /// The same credentials the FTP, SFTP and WebDAV transports use: somebody already
    /// configured that server there, with its password in the vault, and asking them to type
    /// it a second time into an upload setting would be two places to keep in step and two
    /// places for it to leak from.
    async fn remote_login(&self, destination: &str) -> Option<rd_core::RemoteCredential> {
        let url = url::Url::parse(destination).ok()?;
        let host = url.host_str()?.to_ascii_lowercase();
        let port = url.port_or_known_default();
        let credentials = self.database.list_remote_credentials().await.ok()?;
        credentials.into_iter().find(|credential| {
            credential.host.eq_ignore_ascii_case(&host)
                && port.is_none_or(|port| credential.port == port)
        })
    }

    /// Whether a plugin id names an installed post-processing step.
    fn plugin_step_installed(&self, plugin_id: &str) -> bool {
        self.plugin_steps
            .as_ref()
            .is_some_and(|runner| runner.installed(plugin_id))
    }

    /// Scripts directory from the settings (or the configured default); created on demand.
    async fn scripts_directory(&self, settings: &rd_core::PostprocessSettings) -> Result<PathBuf> {
        let directory = settings
            .scripts_directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.config.default_scripts_directory.clone());
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create scripts directory {}", directory.display()))?;
        Ok(directory)
    }
}

/// Cloneable handle; post-processing runs sequentially on a background task.
#[derive(Clone)]
pub struct ExtractionService {
    inner: Arc<Inner>,
}

impl ExtractionService {
    /// Starts the job loop and the completion listener.
    #[must_use]
    pub fn start(database: Database, config: ExtractionConfig) -> Self {
        Self::start_with_plugins(database, config, None, None)
    }

    /// The same, with the installed post-processing steps and upload destinations.
    ///
    /// Separate rather than more parameters everywhere: a service without plugins is the
    /// normal case, and the existing callers (tests included) should not have to say `None`.
    pub fn start_with_plugins(
        database: Database,
        config: ExtractionConfig,
        plugin_steps: Option<Arc<dyn plugin_step::PluginStepRunner>>,
        storage: Option<Arc<dyn storage_upload::StorageUploader>>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(256);
        let service = Self {
            inner: Arc::new(Inner {
                database,
                hold: config.hold.clone(),
                config,
                jobs: sender,
                in_flight: Mutex::new(HashSet::new()),
                shutdown: CancellationToken::new(),
                plugin_steps,
                storage,
            }),
        };
        tokio::spawn(service.clone().run_jobs(receiver));
        tokio::spawn(service.clone().listen_for_completions());
        service
    }

    /// Queues one package; duplicates while a job is pending are ignored.
    pub async fn request(&self, package_id: PackageId, trigger: ExtractionTrigger) -> Result<()> {
        {
            let mut in_flight = self.inner.in_flight.lock().await;
            if trigger == ExtractionTrigger::Auto && in_flight.contains(&package_id) {
                return Ok(());
            }
            in_flight.insert(package_id);
        }
        self.inner
            .jobs
            .send(Job {
                package_id,
                trigger,
            })
            .await
            .context("post-processing service is not running")
    }

    /// Packages waiting for or running in the pipeline.
    pub async fn pending(&self) -> HashSet<PackageId> {
        self.inner.in_flight.lock().await.clone()
    }

    /// Effective scripts directory (settings override or default), created on demand.
    pub async fn scripts_directory(&self) -> Result<PathBuf> {
        let settings = load_postprocess_settings(&self.inner.database).await?;
        self.inner.scripts_directory(&settings).await
    }

    /// Runs a queue-completion script (RD-050-13) through the same sandbox as the
    /// post-processing scripts: the same directory, the same name validation and the same
    /// timeout. It belongs to no package, so nothing is written to the pipeline tables.
    pub async fn run_completion_script(&self, name: &str) -> Result<bool> {
        self.run_named_script(name, &StandaloneScript::default())
            .await
    }

    /// Runs one script from the scripts directory outside the pipeline.
    ///
    /// Shared by the queue-completion action and by automation script actions, so there is
    /// exactly one place that decides where a script may live, how long it may run and how
    /// much of its output is kept. An automation must not be able to reach past that.
    pub async fn run_named_script(&self, name: &str, about: &StandaloneScript) -> Result<bool> {
        let settings = load_postprocess_settings(&self.inner.database).await?;
        let directory = self.inner.scripts_directory(&settings).await?;
        let script = script_job::resolve_script(&directory, name)?;
        let context = script_job::ScriptContext {
            package_id: about.package_id.clone(),
            package_name: if about.package_name.is_empty() {
                "queue".to_owned()
            } else {
                about.package_name.clone()
            },
            final_dir: about.final_dir.clone().unwrap_or_else(|| directory.clone()),
            category: about.category.clone(),
            kind: if about.kind.is_empty() {
                "queue".to_owned()
            } else {
                about.kind.clone()
            },
            status: 0,
        };
        let timeout = std::time::Duration::from_secs(u64::from(settings.script_timeout_seconds));
        let (ok, output) = script_job::execute(&script, &directory, &context, timeout).await?;
        if !ok {
            tracing::warn!(script = name, output = %output, "script failed");
        }
        Ok(ok)
    }

    /// Runs a script from the scripts directory and returns everything it printed
    /// (RD-130-19), for a subscription that reads links from it.
    ///
    /// The same sandbox as [`Self::run_named_script`] -- the same directory, name validation
    /// and timeout, no shell -- with the output limit enforced as a refusal instead of a cut,
    /// and a non-zero exit as a failure with its reason. The script runs in the scripts
    /// directory and learns `RD_KIND=subscription` and `RD_SCRIPT_DIR`, plus whatever the
    /// caller adds to `environment`.
    pub async fn run_output_script(
        &self,
        name: &str,
        mut environment: Vec<(String, String)>,
    ) -> Result<String> {
        let settings = load_postprocess_settings(&self.inner.database).await?;
        let directory = self.inner.scripts_directory(&settings).await?;
        let script = script_job::resolve_script(&directory, name)?;
        environment.push(("RD_KIND".to_owned(), "subscription".to_owned()));
        environment.push((
            "RD_SCRIPT_DIR".to_owned(),
            directory.to_string_lossy().into_owned(),
        ));
        let timeout = std::time::Duration::from_secs(u64::from(settings.script_timeout_seconds));
        script_job::execute_for_output(&script, &directory, environment, timeout).await
    }

    /// Re-queues packages whose pipeline was interrupted.
    ///
    /// A queued step is the usual sign, and a package still recorded as `Postprocessing` is the
    /// other one: nothing is running at startup, so that state can only have been left behind by
    /// an interrupted run. Without it, a package killed between re-queueing its PAR2 volumes and
    /// recording the wait (RD-107-04) would keep downloading them and then have nobody left to
    /// ask for the repair.
    pub async fn recover(&self) -> Result<()> {
        for package in self.inner.database.list_packages().await? {
            let steps = self
                .inner
                .database
                .list_postprocess_steps(&package.id.to_string())
                .await?;
            if package.state == rd_core::PackageState::Postprocessing
                || steps
                    .iter()
                    .any(|step| step.state == PostprocessState::Queued)
            {
                self.request(package.id, ExtractionTrigger::Auto).await?;
            }
        }
        Ok(())
    }

    /// Stops accepting jobs; the running job finishes its current step.
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    async fn run_jobs(self, mut receiver: mpsc::Receiver<Job>) {
        loop {
            let job = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                job = receiver.recv() => match job {
                    Some(job) => job,
                    None => return,
                },
            };
            // Quiet hours only postpone: the job stays queued and starts as soon as the
            // window ends, and a job already running is never interrupted.
            while self.inner.config.quiet_hold.is_held() {
                tokio::select! {
                    () = self.inner.shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(15)) => {}
                }
            }
            // The same trace the download carried, derived the same way (RD-110-03): a
            // package's post-processing and the files it is made of answer one question, so
            // they belong under one id even though nothing was passed between them.
            let trace = rd_core::TraceContext::for_job("package", &job.package_id.to_string());
            let span = tracing::info_span!(
                "postprocess.package",
                trace_id = %trace.trace_id_hex(),
                package_id = %job.package_id,
                trigger = ?job.trigger,
            );
            let result = tracing::Instrument::instrument(
                package_job::run_package(&self.inner, job.package_id, job.trigger),
                span,
            )
            .await;
            self.inner.in_flight.lock().await.remove(&job.package_id);
            if let Err(error) = result {
                tracing::warn!(package_id = %job.package_id, %error, "package post-processing failed");
                let _ = self
                    .inner
                    .database
                    .set_package_state(
                        job.package_id,
                        rd_core::PackageState::Failed,
                        None,
                        None,
                        None,
                    )
                    .await;
            }
        }
    }

    async fn listen_for_completions(self) {
        let mut events = self.inner.database.subscribe();
        loop {
            let event = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                event = events.recv() => match event {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            if event.kind != EventKind::DownloadState
                || !matches!(
                    event.payload.get("state").and_then(|value| value.as_str()),
                    // Seeding torrents have a complete payload, so postprocessing starts
                    // while they keep uploading. A mirror standing down is the last thing
                    // that can settle a package, so it has to wake the check too — without
                    // it the pipeline waits for an event that will never come.
                    Some("completed" | "seeding" | "failed" | "blocked" | "cancelled" | "skipped")
                )
            {
                continue;
            }
            let Some(download_id) = event
                .payload
                .get("download_id")
                .and_then(|value| value.as_str())
                .and_then(|value| value.parse::<rd_core::DownloadId>().ok())
            else {
                continue;
            };
            if let Err(error) = self.on_download_finished(download_id).await {
                tracing::debug!(%error, "post-processing trigger check failed");
            }
        }
    }

    /// Requests the pipeline once every file of the package reached a terminal state
    /// and at least one file completed.
    async fn on_download_finished(&self, download_id: rd_core::DownloadId) -> Result<()> {
        let Some(download) = self.inner.database.get_download(download_id).await? else {
            return Ok(());
        };
        let siblings = self
            .inner
            .database
            .downloads_for_package(download.package_id)
            .await?;
        let all_terminal = siblings.iter().all(|item| {
            matches!(
                item.state,
                DownloadState::Completed
                    | DownloadState::Seeding
                    | DownloadState::Failed
                    | DownloadState::Blocked
                    | DownloadState::Cancelled
                    // A mirror that was never needed is as settled as one that failed.
                    | DownloadState::Skipped
            )
        });
        let any_completed = siblings.iter().any(|item| {
            matches!(
                item.state,
                DownloadState::Completed | DownloadState::Seeding
            )
        });
        if !all_terminal || !any_completed {
            return Ok(());
        }
        let already = self
            .inner
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == download.package_id)
            .is_some_and(|package| {
                matches!(
                    package.state,
                    rd_core::PackageState::Postprocessing
                        | rd_core::PackageState::Completed
                        | rd_core::PackageState::Failed
                )
            });
        if already {
            return Ok(());
        }
        self.request(download.package_id, ExtractionTrigger::Auto)
            .await
    }
}

/// What a script run outside the post-processing pipeline is told about.
///
/// Every field is optional in effect: a queue-completion script has no package, an
/// automation script may or may not have one, and the runner fills in the scripts directory
/// where a package directory would otherwise be.
#[derive(Clone, Debug, Default)]
pub struct StandaloneScript {
    pub package_id: String,
    pub package_name: String,
    pub final_dir: Option<std::path::PathBuf>,
    pub category: Option<String>,
    /// What kind of run this is, passed through to the script as `RD_KIND`.
    pub kind: String,
}

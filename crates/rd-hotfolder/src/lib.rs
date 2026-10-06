//! Reconciled daemon and capture-agent hotfolder watchers.

#![warn(unreachable_pub)]

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rd_core::{HotFolderConfig, ImportMode};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod poll_interval;
mod scanner;

pub use poll_interval::PollInterval;
use scanner::{Scanner, checked_destination, destination_path, dunce_path};

const DEFAULT_RECONCILIATION: Duration = Duration::from_secs(30);
const DEFAULT_STABILITY: Duration = Duration::from_secs(2);
const MAX_INTAKE_BYTES: u64 = 64 * 1024 * 1024;

/// Stable file forwarded by a watcher to the collector or queue.
#[derive(Clone, Debug)]
pub struct HotFolderIntake {
    pub source_path: PathBuf,
    pub sha256: String,
    pub content: Vec<u8>,
    pub mode: ImportMode,
    pub category_id: Option<rd_core::CategoryId>,
}

/// A drop the sink refused, with the reason, so somebody can find it again.
///
/// Nobody watches a watched folder. The log line the scanner writes reaches whoever reads the
/// service log at the right moment and nobody else, and the file itself lands in `failed/`
/// under a name that says nothing about why — so this is handed back to the sink, which is the
/// only party with somewhere durable to put it.
#[derive(Clone, Debug)]
pub struct FailedIntake {
    pub source_path: PathBuf,
    pub sha256: String,
    /// Where the file is moved to, so the reason and the file can be brought together.
    pub failed_path: PathBuf,
    /// Why it could not be taken in, as one line of English.
    pub reason: String,
}

/// The stable code a drop whose content was already taken in is logged under.
pub const DUPLICATE_CODE: &str = "hotfolder.duplicate";

/// A drop whose content this watcher handed over and has not moved away yet, moved to
/// `processed` without a second import.
///
/// A digest is remembered only while its file is in flight — from the import until the file
/// has been moved — so a file whose move failed after its import is not imported again on the
/// next pass. It used to be remembered for as long as the watcher ran, which sent a file
/// somebody dropped again on purpose, after deleting the package, to `processed` without a
/// word; that drop is imported again now, and the NZB history decides whether it is a real
/// duplicate (audit 1.9.1, INTAKE-16).
#[derive(Clone, Debug)]
pub struct DuplicateIntake {
    pub source_path: PathBuf,
    pub sha256: String,
    /// Where the file is moved to.
    pub processed_path: PathBuf,
}

/// Service boundary used by daemon and capture-agent watchers.
#[async_trait]
pub trait IntakeSink: Send + Sync + 'static {
    async fn submit(&self, intake: HotFolderIntake) -> Result<()>;

    /// Records a drop that [`submit`](Self::submit) refused.
    ///
    /// The default does nothing, for a sink with nowhere to record it; the scanner has already
    /// logged the failure and moved the file aside by the time this is called.
    async fn record_failure(&self, _failure: FailedIntake) {}

    /// Records a drop that was not submitted because its content already was.
    ///
    /// The default does nothing; the scanner has already logged it under [`DUPLICATE_CODE`].
    async fn record_duplicate(&self, _duplicate: DuplicateIntake) {}
}

/// Runtime controls for mandatory polling and stable-file detection.
#[derive(Clone, Debug)]
pub struct WatchOptions {
    pub reconciliation_interval: PollInterval,
    pub stability_window: Duration,
    /// The first wait after the folder itself failed (missing, unreadable, unmounted); each
    /// further failure in a row doubles it up to [`MAX_RETRY_DELAY`].
    pub retry_delay: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            reconciliation_interval: PollInterval::default(),
            stability_window: DEFAULT_STABILITY,
            retry_delay: DEFAULT_RETRY_DELAY,
        }
    }
}

const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(5);
/// The longest a failed folder waits before it is tried again.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// Starts one native watcher plus an unconditional reconciliation scan.
///
/// The task ends with an error only for a configuration that can never work (an empty path, a
/// destination outside the folder); a folder that fails at run time is retried with a backoff
/// until the token is cancelled.
pub fn spawn(
    config: HotFolderConfig,
    sink: Arc<dyn IntakeSink>,
    cancellation: CancellationToken,
    options: WatchOptions,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move { run(config, sink, cancellation, options).await })
}

async fn run(
    config: HotFolderConfig,
    sink: Arc<dyn IntakeSink>,
    cancellation: CancellationToken,
    options: WatchOptions,
) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }
    let root = dunce_path(&config.path)?;
    destination_path(&root, &config.processed_path)?;
    destination_path(&root, &config.failed_path)?;
    // Kept across attempts, so a folder that comes back does not import a file twice.
    let mut imported = HashSet::new();
    let mut delay = options.retry_delay;
    loop {
        let mut healthy = false;
        let error = match watch(
            &config,
            &root,
            &sink,
            &cancellation,
            &options,
            &mut imported,
            &mut healthy,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if healthy {
            delay = options.retry_delay;
        }
        // A folder that stops working used to end this task for good, with nothing but a dead
        // `JoinHandle` to say so; a network share that is back after a reboot was never
        // watched again until the service restarted.
        tracing::error!(
            error = %format!("{error:#}"),
            folder = %config.name,
            path = %root.display(),
            retry_in_seconds = delay.as_secs(),
            "hotfolder cannot watch this folder; it is tried again"
        );
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            () = tokio::time::sleep(delay) => {}
        }
        delay = delay.saturating_mul(2).min(MAX_RETRY_DELAY);
    }
}

/// One attempt at watching the folder: `Ok` when cancelled, an error when the folder itself
/// failed. `healthy` turns true once a full scan went through, which resets the backoff.
async fn watch(
    config: &HotFolderConfig,
    root: &Path,
    sink: &Arc<dyn IntakeSink>,
    cancellation: &CancellationToken,
    options: &WatchOptions,
    imported: &mut HashSet<String>,
    healthy: &mut bool,
) -> Result<()> {
    tokio::fs::create_dir_all(root).await?;
    let root = dunce::canonicalize(root)?;
    let processed = destination_path(&root, &config.processed_path)?;
    let failed = destination_path(&root, &config.failed_path)?;
    tokio::fs::create_dir_all(&processed).await?;
    tokio::fs::create_dir_all(&failed).await?;
    let processed = checked_destination(&root, processed)?;
    let failed = checked_destination(&root, failed)?;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<PathBuf>();
    let mut watcher = make_watcher(event_tx)?;
    watcher.watch(
        &root,
        if config.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        },
    )?;

    let mut scanner = Scanner {
        config: config.clone(),
        root,
        processed,
        failed,
        sink: Arc::clone(sink),
        stability: options.stability_window,
        observed: HashMap::new(),
        imported,
    };
    scanner.scan().await?;
    *healthy = true;
    // `options` holds the interval's sender for as long as this loop runs, so `changed()`
    // never reports a closed channel here.
    let mut interval = options.reconciliation_interval.subscribe();
    let mut ticker = new_ticker(*interval.borrow_and_update());
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            _ = ticker.tick() => scanner.scan().await?,
            changed = interval.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                ticker = new_ticker(*interval.borrow_and_update());
            }
            path = event_rx.recv() => {
                // The watcher holds the sender, so the channel stays open while it lives.
                let Some(path) = path else { return Ok(()) };
                // One file that cannot be handled - locked by the program still writing it on
                // Windows, a move that failed - is the next pass's business, as in `scan`.
                if let Err(error) = scanner.inspect(path.clone()).await {
                    tracing::warn!(
                        %error,
                        path = %path.display(),
                        "hotfolder could not process this file; it stays for the next pass"
                    );
                }
            }
        }
    }
}

/// A ticker whose first tick is one interval away: the scan at start has just run, and a
/// re-armed ticker must not scan again right away either.
fn new_ticker(period: Duration) -> tokio::time::Interval {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker
}

fn make_watcher(sender: mpsc::UnboundedSender<PathBuf>) -> Result<RecommendedWatcher> {
    let watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                for path in event.paths {
                    let _ = sender.send(path);
                }
            }
            Err(error) => tracing::warn!(%error, "hotfolder event error"),
        })?;
    Ok(watcher)
}

#[cfg(test)]
mod tests;

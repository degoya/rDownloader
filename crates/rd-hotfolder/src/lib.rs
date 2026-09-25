//! Reconciled daemon and capture-agent hotfolder watchers.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rd_core::{HotFolderConfig, ImportMode};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod poll_interval;

pub use poll_interval::PollInterval;

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

/// Service boundary used by daemon and capture-agent watchers.
#[async_trait]
pub trait IntakeSink: Send + Sync + 'static {
    async fn submit(&self, intake: HotFolderIntake) -> Result<()>;

    /// Records a drop that [`submit`](Self::submit) refused.
    ///
    /// The default does nothing, for a sink with nowhere to record it; the scanner has already
    /// logged the failure and moved the file aside by the time this is called.
    async fn record_failure(&self, _failure: FailedIntake) {}
}

/// Runtime controls for mandatory polling and stable-file detection.
#[derive(Clone, Debug)]
pub struct WatchOptions {
    pub reconciliation_interval: PollInterval,
    pub stability_window: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            reconciliation_interval: PollInterval::default(),
            stability_window: DEFAULT_STABILITY,
        }
    }
}

/// Starts one native watcher plus an unconditional reconciliation scan.
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
    tokio::fs::create_dir_all(&root).await?;
    let root = dunce::canonicalize(&root)?;
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

    let mut scanner = Scanner::new(
        config,
        root,
        processed,
        failed,
        sink,
        options.stability_window,
    );
    scanner.scan().await?;
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
                let Some(path) = path else { return Ok(()) };
                scanner.inspect(path).await?;
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

struct Scanner {
    config: HotFolderConfig,
    root: PathBuf,
    processed: PathBuf,
    failed: PathBuf,
    sink: Arc<dyn IntakeSink>,
    stability: Duration,
    observed: HashMap<PathBuf, Observation>,
    imported: HashSet<String>,
}

#[derive(Clone, Copy)]
struct Observation {
    length: u64,
    modified: SystemTime,
    unchanged_since: tokio::time::Instant,
}

impl Scanner {
    fn new(
        config: HotFolderConfig,
        root: PathBuf,
        processed: PathBuf,
        failed: PathBuf,
        sink: Arc<dyn IntakeSink>,
        stability: Duration,
    ) -> Self {
        Self {
            config,
            root,
            processed,
            failed,
            sink,
            stability,
            observed: HashMap::new(),
            imported: HashSet::new(),
        }
    }

    /// One pass over the folder. A file that cannot be handled is logged and left where it is.
    ///
    /// Propagating a per-file error from here ended the watcher task for good: a file that
    /// vanished between the stability check and the read, a full `processed` directory or one
    /// unreadable entry stopped the folder importing anything until the service was
    /// restarted, with nothing but a dead `JoinHandle` to say so.
    async fn scan(&mut self) -> Result<()> {
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

    async fn inspect(&mut self, path: PathBuf) -> Result<()> {
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
            move_verified(&path, &unique_destination(&self.processed, &path), &sha256).await?;
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
        move_verified(&path, &destination, &sha256).await
    }
}

/// Every candidate file below `root`.
///
/// The root has to be readable — if it is not, the watch is misconfigured and the caller
/// should hear about it. A single sub-directory that is not is skipped instead, so one
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

async fn move_verified(source: &Path, destination: &Path, expected_hash: &str) -> Result<()> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::rename(source, destination).await.is_ok() {
        return Ok(());
    }
    tokio::fs::copy(source, destination).await?;
    let copied = tokio::fs::read(destination).await?;
    if hex::encode(Sha256::digest(&copied)) != expected_hash {
        bail!("copy verification failed for {}", destination.display());
    }
    tokio::fs::remove_file(source).await?;
    Ok(())
}

/// The error chain as one bounded line.
///
/// Bounded because this is stored and shown: an error that carries a parser dump or a server
/// answer would otherwise put an unbounded blob into the database and into the list that shows
/// it. Newlines collapse so the reason stays one line in a row that has room for one.
fn reason(error: &anyhow::Error) -> String {
    let flattened = format!("{error:#}")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match flattened.char_indices().nth(MAX_REASON_CHARS) {
        Some((index, _)) => format!("{}...", &flattened[..index]),
        None => flattened,
    }
}

const MAX_REASON_CHARS: usize = 500;

fn unique_destination(directory: &Path, source: &Path) -> PathBuf {
    let name = source.file_name().unwrap_or_default();
    let direct = directory.join(name);
    if !direct.exists() {
        return direct;
    }
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("import");
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("nzb");
    for index in 1_u32.. {
        let candidate = directory.join(format!("{stem} ({index}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an available filename exists")
}

fn destination_path(root: &Path, configured: &str) -> Result<PathBuf> {
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

fn checked_destination(root: &Path, path: PathBuf) -> Result<PathBuf> {
    let canonical = dunce::canonicalize(&path)?;
    if !canonical.starts_with(root) {
        bail!("hotfolder destination escapes through a symlink");
    }
    Ok(canonical)
}

fn dunce_path(value: &str) -> Result<PathBuf> {
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

fn is_candidate(path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use anyhow::Result;
    use async_trait::async_trait;
    use rd_core::{HotFolderConfig, HotFolderExecutor, HotFolderId, ImportMode};
    use sha2::{Digest, Sha256};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::{
        FailedIntake, HotFolderIntake, IntakeSink, PollInterval, WatchOptions, is_candidate, spawn,
    };

    struct ChannelSink(mpsc::Sender<HotFolderIntake>);

    /// A sink that refuses everything, the way the real one refuses an NZB that will not parse.
    struct RefusingSink(mpsc::Sender<FailedIntake>);

    #[async_trait]
    impl IntakeSink for RefusingSink {
        async fn submit(&self, _intake: HotFolderIntake) -> Result<()> {
            Err(anyhow::anyhow!("NZB could not be parsed").context("hotfolder intake"))
        }

        async fn record_failure(&self, failure: FailedIntake) {
            let _ = self.0.send(failure).await;
        }
    }

    fn config(directory: &std::path::Path) -> HotFolderConfig {
        HotFolderConfig {
            id: HotFolderId::new(),
            name: "test".to_owned(),
            executor: HotFolderExecutor::Daemon,
            path: directory.to_string_lossy().into_owned(),
            recursive: false,
            category_id: None,
            import_mode: ImportMode::Review,
            processed_path: "processed".to_owned(),
            failed_path: "failed".to_owned(),
            enabled: true,
        }
    }

    fn options() -> WatchOptions {
        WatchOptions {
            reconciliation_interval: PollInterval::new(Duration::from_millis(20)),
            stability_window: Duration::from_millis(30),
        }
    }

    /// RD-110-31: the interval is a setting now, and a changed setting reaches a running
    /// watcher. The file is there before the watch starts, so no native event ever mentions
    /// it: the first scan observes it, and only a second look can find it stable. With an
    /// hour between scans that look never comes; once the interval is lowered it does, and
    /// the file is imported without a restart.
    #[tokio::test]
    async fn a_changed_interval_is_taken_without_a_restart() {
        let directory = tempfile::tempdir().expect("temporary directory");
        tokio::fs::write(directory.path().join("sample.nzb"), b"<nzb/>")
            .await
            .expect("write NZB");
        let (sender, mut receiver) = mpsc::channel(1);
        let cancellation = CancellationToken::new();
        let interval = PollInterval::new(Duration::from_secs(3600));
        let handle = spawn(
            config(directory.path()),
            Arc::new(ChannelSink(sender)),
            cancellation.clone(),
            WatchOptions {
                reconciliation_interval: interval.clone(),
                stability_window: Duration::from_millis(30),
            },
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(300), receiver.recv())
                .await
                .is_err(),
            "imported although the next scan is an hour away"
        );

        interval.set(Duration::from_millis(20));
        let intake = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("imported once the interval was lowered")
            .expect("intake");
        assert_eq!(intake.content, b"<nzb/>");
        cancellation.cancel();
        handle.await.expect("watcher task").expect("watcher result");
    }

    #[test]
    fn a_reason_is_one_bounded_line() {
        let error = anyhow::anyhow!("line one\nline two").context("outer");
        assert_eq!(super::reason(&error), "outer: line one line two");
        let long = anyhow::anyhow!("x".repeat(super::MAX_REASON_CHARS + 50));
        let reason = super::reason(&long);
        assert_eq!(reason.chars().count(), super::MAX_REASON_CHARS + 3);
        assert!(reason.ends_with("..."), "{reason}");
    }

    /// RD-108-20: an NZB nobody threw in by hand used to reach nobody at all. The watcher hands
    /// the refusal back to the sink, which is what puts it where the interface can show it.
    #[tokio::test]
    async fn a_drop_that_cannot_be_taken_in_is_reported_to_the_sink() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (sender, mut receiver) = mpsc::channel(1);
        let cancellation = CancellationToken::new();
        let handle = spawn(
            config(directory.path()),
            Arc::new(RefusingSink(sender)),
            cancellation.clone(),
            options(),
        );
        tokio::fs::write(directory.path().join("broken.nzb"), b"not an NZB at all")
            .await
            .expect("write NZB");

        let failure = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("receive before timeout")
            .expect("failure");

        assert_eq!(
            failure.source_path,
            dunce::canonicalize(directory.path())
                .expect("canonical root")
                .join("broken.nzb")
        );
        assert_eq!(
            failure.sha256,
            hex::encode(Sha256::digest(b"not an NZB at all"))
        );
        assert_eq!(
            failure.failed_path.file_name().and_then(|n| n.to_str()),
            Some("broken.nzb")
        );
        assert_eq!(failure.reason, "hotfolder intake: NZB could not be parsed");

        let failed = directory.path().join("failed/broken.nzb");
        tokio::time::timeout(Duration::from_secs(1), async {
            while !failed.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("failed move");
        cancellation.cancel();
        handle.await.expect("watcher task").expect("watcher result");
    }

    #[test]
    fn accepts_container_files_only() {
        assert!(is_candidate(std::path::Path::new("package.nzb")));
        assert!(is_candidate(std::path::Path::new("package.TORRENT")));
        assert!(is_candidate(std::path::Path::new("package.dlc")));
        assert!(is_candidate(std::path::Path::new("package.ccf")));
        assert!(is_candidate(std::path::Path::new("package.RSDF")));
        // A watched folder is somewhere people also keep notes.
        assert!(!is_candidate(std::path::Path::new("README.txt")));
        assert!(!is_candidate(std::path::Path::new("package.zip")));
        assert!(!is_candidate(std::path::Path::new(".package.torrent")));
        assert!(!is_candidate(std::path::Path::new("package.torrent.part")));
    }

    #[async_trait]
    impl IntakeSink for ChannelSink {
        async fn submit(&self, intake: HotFolderIntake) -> Result<()> {
            self.0.send(intake).await?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn reconciliation_imports_a_stable_nzb_and_moves_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (sender, mut receiver) = mpsc::channel(1);
        let cancellation = CancellationToken::new();
        let handle = spawn(
            config(directory.path()),
            Arc::new(ChannelSink(sender)),
            cancellation.clone(),
            options(),
        );
        tokio::fs::write(directory.path().join("sample.nzb"), b"<nzb/>")
            .await
            .expect("write NZB");
        let intake = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("receive before timeout")
            .expect("intake");
        assert_eq!(intake.content, b"<nzb/>");
        let processed = directory.path().join("processed/sample.nzb");
        tokio::time::timeout(Duration::from_secs(1), async {
            while !processed.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("processed move");
        assert!(processed.exists());
        cancellation.cancel();
        handle.await.expect("watcher task").expect("watcher result");
    }
}

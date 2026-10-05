use std::{sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{HotFolderConfig, HotFolderExecutor, HotFolderId, ImportMode};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::scanner::is_candidate;
use super::{
    DuplicateIntake, FailedIntake, HotFolderIntake, IntakeSink, PollInterval, WatchOptions, spawn,
};

struct ChannelSink(mpsc::Sender<HotFolderIntake>);

/// A sink that takes everything and reports what it was told about duplicates.
struct DuplicateSink {
    submitted: mpsc::Sender<HotFolderIntake>,
    duplicates: mpsc::Sender<DuplicateIntake>,
}

#[async_trait]
impl IntakeSink for DuplicateSink {
    async fn submit(&self, intake: HotFolderIntake) -> Result<()> {
        self.submitted.send(intake).await?;
        Ok(())
    }

    async fn record_duplicate(&self, duplicate: DuplicateIntake) {
        let _ = self.duplicates.send(duplicate).await;
    }
}

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
        retry_delay: Duration::from_millis(20),
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
            retry_delay: Duration::from_millis(20),
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
    assert_eq!(super::scanner::reason(&error), "outer: line one line two");
    let long = anyhow::anyhow!("x".repeat(super::scanner::MAX_REASON_CHARS + 50));
    let reason = super::scanner::reason(&long);
    assert_eq!(reason.chars().count(), super::scanner::MAX_REASON_CHARS + 3);
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

/// Audit 1.9.1, INTAKE-16: the same content dropped again after the first file was
/// imported and moved away is a deliberate second drop and is imported again; only a file
/// still in flight counts as a duplicate here.
#[tokio::test]
async fn a_file_dropped_again_after_its_import_finished_is_imported_again() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (submitted, mut submissions) = mpsc::channel(4);
    let (duplicates, mut reported) = mpsc::channel(4);
    let cancellation = CancellationToken::new();
    let handle = spawn(
        config(directory.path()),
        Arc::new(DuplicateSink {
            submitted,
            duplicates,
        }),
        cancellation.clone(),
        options(),
    );
    tokio::fs::write(directory.path().join("sample.nzb"), b"<nzb/>")
        .await
        .expect("write NZB");
    tokio::time::timeout(Duration::from_secs(2), submissions.recv())
        .await
        .expect("first import before timeout")
        .expect("intake");
    wait_for(&directory.path().join("processed/sample.nzb")).await;

    tokio::fs::write(directory.path().join("again.nzb"), b"<nzb/>")
        .await
        .expect("write the NZB again");
    let second = tokio::time::timeout(Duration::from_secs(2), submissions.recv())
        .await
        .expect("second import before timeout")
        .expect("intake");
    assert_eq!(second.sha256, hex::encode(Sha256::digest(b"<nzb/>")));
    wait_for(&directory.path().join("processed/again.nzb")).await;
    assert!(
        reported.try_recv().is_err(),
        "a finished import is no duplicate in flight"
    );
    cancellation.cancel();
    handle.await.expect("watcher task").expect("watcher result");
}

async fn wait_for(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} did not appear", path.display()));
}

/// A file the watcher cannot read - on Windows, one the program writing it still holds
/// locked - ended the whole watcher when a native event, not the scan, found it stable.
/// The reconciliation interval is an hour here, so only events drive the watcher.
#[cfg(unix)]
#[tokio::test]
async fn a_file_that_cannot_be_read_does_not_stop_the_watcher() {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    let directory = tempfile::tempdir().expect("temporary directory");
    let (sender, mut receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    let interval = PollInterval::new(Duration::from_secs(3600));
    let handle = spawn(
        config(directory.path()),
        Arc::new(ChannelSink(sender)),
        cancellation.clone(),
        WatchOptions {
            reconciliation_interval: interval.clone(),
            stability_window: Duration::ZERO,
            retry_delay: Duration::from_secs(3600),
        },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let locked = directory.path().join("locked.nzb");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o000)
        .open(&locked)
        .expect("create unreadable NZB");
    file.write_all(b"<nzb/>").expect("write NZB");
    drop(file);
    if std::fs::read(&locked).is_ok() {
        // Running as root: nothing can be made unreadable, so there is nothing to test.
        cancellation.cancel();
        return;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !handle.is_finished(),
        "the watcher ended on one unreadable file"
    );

    // Scans from here on, so the next file does not depend on how many events a
    // platform coalesces one write into.
    interval.set(Duration::from_millis(20));
    tokio::fs::write(directory.path().join("sample.nzb"), b"<nzb/>")
        .await
        .expect("write NZB");
    let intake = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("imported after the unreadable file")
        .expect("intake");
    assert!(intake.source_path.ends_with("sample.nzb"));
    cancellation.cancel();
    handle.await.expect("watcher task").expect("watcher result");
}

/// A folder that is not there yet (an unmounted share, a path below a file) is tried again
/// with a backoff instead of ending the watcher, and watched once it works.
#[tokio::test]
async fn a_folder_that_fails_is_watched_once_it_works() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let blocker = directory.path().join("share");
    tokio::fs::write(&blocker, b"not a directory")
        .await
        .expect("write blocker");
    let root = blocker.join("watch");
    let (sender, mut receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    let handle = spawn(
        config(&root),
        Arc::new(ChannelSink(sender)),
        cancellation.clone(),
        options(),
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!handle.is_finished(), "the watcher gave up on the folder");

    tokio::fs::remove_file(&blocker)
        .await
        .expect("remove blocker");
    wait_for(&root.join("processed")).await;
    tokio::fs::write(root.join("sample.nzb"), b"<nzb/>")
        .await
        .expect("write NZB");
    let intake = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("imported once the folder works")
        .expect("intake");
    assert_eq!(intake.content, b"<nzb/>");
    cancellation.cancel();
    handle.await.expect("watcher task").expect("watcher result");
}

/// A configuration that can never work still ends the task, with the reason.
#[tokio::test]
async fn a_destination_outside_the_folder_ends_the_watcher() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut config = config(directory.path());
    config.processed_path = "../elsewhere".to_owned();
    let (sender, _receiver) = mpsc::channel(1);
    let handle = spawn(
        config,
        Arc::new(ChannelSink(sender)),
        CancellationToken::new(),
        options(),
    );
    let result = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("ends at once")
        .expect("watcher task");
    assert!(result.is_err());
}

/// The name in `processed/` is taken by an earlier file: the new one gets ` (1)`, and the
/// earlier one keeps its bytes.
#[tokio::test]
async fn a_taken_name_is_never_overwritten() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let processed = directory.path().join("processed");
    tokio::fs::create_dir_all(&processed)
        .await
        .expect("processed directory");
    tokio::fs::write(processed.join("sample.nzb"), b"earlier")
        .await
        .expect("write earlier NZB");
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
    tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("receive before timeout")
        .expect("intake");
    wait_for(&processed.join("sample (1).nzb")).await;
    assert_eq!(
        tokio::fs::read(processed.join("sample.nzb"))
            .await
            .expect("earlier NZB"),
        b"earlier"
    );
    cancellation.cancel();
    handle.await.expect("watcher task").expect("watcher result");
}

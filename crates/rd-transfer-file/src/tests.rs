//! The shared staging half against a real database and a real disk.

use rd_core::{DownloadFile, DownloadKind, DownloadState, StorageRootId};
use rd_db::{Database, NewDownload, NewPackage};
use rd_files::StorageRoot;
use rd_scheduler::RunOutcome;
use tokio::io::AsyncWriteExt;

use super::{Labels, Resume, Staging, TransferEnd};

/// The labels are the caller's; any pair of constants exercises the shared code.
const LABELS: Labels = Labels {
    length_mismatch: "test.length_mismatch",
    length_mismatch_message: "The transfer did not deliver the expected number of bytes",
    stalled: "the test server stopped sending data",
    open_part: "open the partial test download",
    flush_part: "flush the test download",
};

struct Fixture {
    _directory: tempfile::TempDir,
    database: Database,
    root: StorageRoot,
    file: DownloadFile,
    part_path: std::path::PathBuf,
}

impl Fixture {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("staging.sqlite3"))
            .await
            .expect("database");
        let destination = directory.path().join("downloads");
        tokio::fs::create_dir_all(&destination).await.expect("dir");
        let root = StorageRoot::create(
            StorageRootId::new(),
            "download destination".to_owned(),
            destination,
        )
        .await
        .expect("root");
        let package_id = rd_core::PackageId::new();
        database
            .create_package(NewPackage {
                id: package_id,
                name: "staging".to_owned(),
                destination: root.path().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let file = database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id,
                source: "ftp://127.0.0.1/pub/movie.bin".parse().expect("url"),
                file_name: "movie.bin".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Ftp,
                media: None,
                remote_credential_id: None,
                mirror_group: None,
                replay: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        let part_path = rd_files::part_path(&root, file.id)
            .await
            .expect("part path");
        Self {
            _directory: directory,
            database,
            root,
            file,
            part_path,
        }
    }

    async fn staging(&self, size: u64) -> Staging<'_> {
        Staging::open(
            &self.database,
            &self.file,
            &self.root,
            &self.part_path,
            size,
            LABELS,
        )
        .await
    }

    fn final_path(&self) -> std::path::PathBuf {
        self.root.path().join("movie.bin")
    }
}

/// Writes `bytes` into the staging file and hands back the open handle, the way a
/// finished read loop leaves it.
async fn part_with(fixture: &Fixture, bytes: &[u8]) -> tokio::fs::File {
    let staging = fixture.staging(0).await;
    let mut sink = staging.open_part().await.expect("open part");
    sink.write_all(bytes).await.expect("write");
    sink
}

#[tokio::test]
async fn a_different_version_refuses_the_resume() {
    let fixture = Fixture::start().await;
    let fresh = fixture.staging(1000).await;
    assert!(matches!(
        fresh
            .plan_resume_validated(Some("\"v1\"".to_owned()), None)
            .await
            .expect("plan"),
        Resume::Fresh
    ));
    let sink = part_with(&fixture, &[7u8; 500]).await;
    sink.sync_all().await.expect("sync");

    // Same size, same time, another object behind the same key: appending its bytes to
    // the old ones would publish a file that is neither.
    let staging = fixture.staging(1000).await;
    assert!(matches!(
        staging
            .plan_resume_validated(Some("\"v2\"".to_owned()), None)
            .await
            .expect("plan"),
        Resume::Refused
    ));
    assert!(matches!(
        staging
            .plan_resume_validated(Some("\"v1\"".to_owned()), None)
            .await
            .expect("plan"),
        Resume::Continue
    ));
}

/// TR-03: a timestamp recorded for the bytes on disk that the server no longer reports — an
/// `MDTM` that failed this time — confirms nothing. Size alone used to be enough.
#[tokio::test]
async fn a_recorded_timestamp_the_server_no_longer_reports_refuses_the_resume() {
    const STAMP: &str = "2026-01-01T12:00:00+00:00";
    let fixture = Fixture::start().await;
    let fresh = fixture.staging(1000).await;
    assert!(matches!(
        fresh
            .plan_resume(Some(STAMP.to_owned()))
            .await
            .expect("plan"),
        Resume::Fresh
    ));
    let sink = part_with(&fixture, &[7u8; 500]).await;
    sink.sync_all().await.expect("sync");

    let staging = fixture.staging(1000).await;
    assert!(matches!(
        staging.plan_resume(None).await.expect("plan"),
        Resume::Refused
    ));
    assert!(matches!(
        staging
            .plan_resume(Some(STAMP.to_owned()))
            .await
            .expect("plan"),
        Resume::Continue
    ));
}

#[tokio::test]
async fn a_short_delivery_is_refused_and_the_partial_file_is_kept() {
    let fixture = Fixture::start().await;
    let sink = part_with(&fixture, &[7u8; 900]).await;

    let staging = fixture.staging(1000).await;
    let outcome = staging
        .finish(sink, TransferEnd::Complete)
        .await
        .expect("finish");

    match outcome {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(LABELS.length_mismatch));
        }
        other => panic!("a short delivery must be refused, got {other:?}"),
    }
    // Refusing must not throw away what was downloaded, and must not publish it.
    assert!(fixture.part_path.exists());
    assert!(!fixture.final_path().exists());
}

#[tokio::test]
async fn a_long_delivery_is_refused_rather_than_promoted() {
    let fixture = Fixture::start().await;
    // The defect this pins: a server that acknowledges a resume offset and then streams
    // from byte zero appends a second copy behind the part already on disk. Accepting
    // anything that merely reaches the announced size publishes that as the payload.
    let sink = part_with(&fixture, &[7u8; 1400]).await;

    let staging = fixture.staging(1000).await;
    let outcome = staging
        .finish(sink, TransferEnd::Complete)
        .await
        .expect("finish");

    match outcome {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(LABELS.length_mismatch));
        }
        other => panic!("a long delivery must be refused, got {other:?}"),
    }
    assert!(fixture.part_path.exists());
    assert!(!fixture.final_path().exists());
}

#[tokio::test]
async fn a_complete_delivery_is_synced_before_it_is_promoted() {
    let fixture = Fixture::start().await;
    let payload = vec![7u8; 1000];
    // Deliberately not flushed here: `finish` owns the sync, and the rename that
    // publishes the file must not happen before it. Renaming first would publish a
    // file whose tail is still in a buffer this process has not handed to the kernel.
    let sink = part_with(&fixture, &payload).await;

    let staging = fixture.staging(1000).await;
    let outcome = staging
        .finish(sink, TransferEnd::Complete)
        .await
        .expect("finish");

    match outcome {
        RunOutcome::Completed { final_name } => assert_eq!(final_name, "movie.bin"),
        other => panic!("a complete delivery must be promoted, got {other:?}"),
    }
    let published = tokio::fs::read(fixture.final_path()).await.expect("final");
    assert_eq!(published, payload);
    // The staging file must not survive a completed transfer.
    assert!(!fixture.part_path.exists());
}

/// RA-TR-05: a stop records the part file's length, so a paused row does not lag behind what
/// is on disk by up to one checkpoint interval.
#[tokio::test]
async fn a_stop_records_what_is_on_disk() {
    let fixture = Fixture::start().await;
    let sink = part_with(&fixture, &[7u8; 700]).await;

    let staging = fixture.staging(1000).await;
    let outcome = staging
        .finish(sink, TransferEnd::Stopped)
        .await
        .expect("finish");

    assert!(matches!(outcome, RunOutcome::Stopped), "{outcome:?}");
    let row = fixture
        .database
        .get_download(fixture.file.id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.committed_bytes.get(), 700);
    assert_eq!(row.total_bytes.map(|total| total.get()), Some(1000));
    assert!(fixture.part_path.exists(), "a stop keeps the part file");
    assert!(!fixture.final_path().exists());
}

/// Axis A of the recovery matrix for `transfer_file.before_progress_recorded` (TR-17): bytes
/// synced to the part file but not yet in the row. The next run resumes at the part file's
/// length and fetches nothing twice.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_before_the_progress_is_recorded_resumes_at_the_part_file() {
    use std::time::Duration;

    let fixture = Fixture::start().await;
    let payload: Vec<u8> = (0..9 * 1024 * 1024 + 4321_u32)
        .map(|index| (index % 251) as u8)
        .collect();
    let size = payload.len() as u64;
    let limiter = rd_limits::ScopedLimiter::unlimited();
    let cancellation = tokio_util::sync::CancellationToken::new();

    let first = fixture.staging(size).await;
    assert!(matches!(
        first.plan_resume(None).await.expect("plan"),
        Resume::Fresh
    ));
    let mut sink = first.open_part().await.expect("open");
    let guard = rd_core::failpoint::FailpointGuard::once("transfer_file.before_progress_recorded");
    let mut source: &[u8] = &payload;
    assert!(
        first
            .stream(
                &mut source,
                &mut sink,
                &limiter,
                &cancellation,
                Duration::from_secs(30)
            )
            .await
            .is_err(),
        "the crash point did not stop the transfer"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);
    drop(sink);

    // The state a restart finds, asserted rather than assumed: the row knows less than the
    // part file holds (invariant 1 is about the row, and it claims nothing unsynced).
    let on_disk = rd_files::existing_bytes(&fixture.part_path).await;
    assert!(
        on_disk > 0 && on_disk < size,
        "{on_disk} of {size} bytes on disk"
    );
    let row = fixture
        .database
        .get_download(fixture.file.id)
        .await
        .expect("read")
        .expect("row");
    assert!(row.committed_bytes.get() < on_disk);

    let second = fixture.staging(size).await;
    assert_eq!(second.committed(), on_disk);
    assert!(matches!(
        second.plan_resume(None).await.expect("plan"),
        Resume::Continue
    ));
    let mut sink = second.open_part().await.expect("open");
    let mut rest: &[u8] = &payload[usize::try_from(on_disk).expect("fits")..];
    let end = second
        .stream(
            &mut rest,
            &mut sink,
            &limiter,
            &cancellation,
            Duration::from_secs(30),
        )
        .await
        .expect("stream");
    let outcome = second.finish(sink, end).await.expect("finish");
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    // Invariants 2 and 3: nothing before the resume point was rewritten, and the file is the
    // source byte for byte. Invariant 4: no part file is left.
    assert_eq!(
        tokio::fs::read(fixture.final_path()).await.expect("final"),
        payload
    );
    assert!(!fixture.part_path.exists());
}

/// Every crash point this crate owns has a case in the crate that arms it.
#[test]
fn every_transfer_file_crash_point_is_exercised_by_a_case() {
    let source = crate_sources();
    for point in rd_crash_points::CRASH_POINTS
        .iter()
        .filter(|point| point.owner == "rd-transfer-file")
    {
        assert!(
            source.contains(&format!("FailpointGuard::once(\"{}\")", point.name)),
            "{} is registered but no case in this crate arms it",
            point.name
        );
    }
}

/// Every `.rs` file under this crate's `src/` and `tests/`, read at run time (RD-1100-13): a case
/// may live in any test file, and a new one is found without a list naming it.
fn crate_sources() -> String {
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let mut pending = vec![root.join("src"), root.join("tests")];
    let mut source = String::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("read a source directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                source.push_str(&std::fs::read_to_string(&path).expect("read a source file"));
            }
        }
    }
    source
}

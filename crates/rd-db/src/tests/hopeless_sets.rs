//! A Usenet set given up as beyond repair (RD-1100-02): which rows the write fails, and how
//! the package reads afterwards.

use rd_core::{DownloadState, EventKind, Failure, FailureKind, ImportMode, IngressSource};

use super::nzb_file;
use crate::{Database, NewNzbImport};

const NAMES: [&str; 5] = [
    "release.part1.rar",
    "release.part2.rar",
    "release.part3.rar",
    "release.par2",
    "release.vol000+01.par2",
];

/// One package of three archive volumes, the index and a recovery volume RD-107-04 postpones.
async fn release(
    directory: &std::path::Path,
) -> (Database, rd_core::PackageId, Vec<rd_core::DownloadFile>) {
    let database = Database::open(directory.join("hopeless.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "release.nzb".to_owned(),
            sha256: "c7".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: NAMES.iter().map(|name| nzb_file(name)).collect(),
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows = database
        .downloads_for_package(package.id)
        .await
        .expect("rows");
    (database, package.id, rows)
}

fn id_of(rows: &[rd_core::DownloadFile], name: &str) -> rd_core::DownloadId {
    rows.iter()
        .find(|file| file.file_name == name)
        .map(|file| file.id)
        .unwrap_or_else(|| panic!("no row for {name}"))
}

async fn drive(database: &Database, id: rd_core::DownloadId, states: &[DownloadState]) {
    for state in states {
        database
            .transition_download(id, *state)
            .await
            .expect("transition");
    }
}

async fn state_of(database: &Database, id: rd_core::DownloadId) -> rd_core::DownloadFile {
    database
        .get_download(id)
        .await
        .expect("row")
        .expect("row exists")
}

async fn package_state(database: &Database, id: rd_core::PackageId) -> rd_core::PackageState {
    database
        .get_package(id)
        .await
        .expect("package")
        .expect("package exists")
        .state
}

fn hopeless() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        crate::USENET_JOB_HOPELESS,
        "9 PAR2 blocks are missing and at most 1 are available to repair them",
    )
    .with_param("missing_blocks", 9)
    .with_param("available_blocks", 1)
}

#[tokio::test]
async fn giving_up_fails_what_waits_and_leaves_what_runs_to_its_runner() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, package, rows) = release(directory.path()).await;
    let running = id_of(&rows, "release.part1.rar");
    let paused = id_of(&rows, "release.part2.rar");
    let held = id_of(&rows, "release.part3.rar");
    let queued = id_of(&rows, "release.par2");
    let postponed = id_of(&rows, "release.vol000+01.par2");
    drive(
        &database,
        running,
        &[DownloadState::Resolving, DownloadState::Downloading],
    )
    .await;
    drive(&database, paused, &[DownloadState::Paused]).await;
    drive(
        &database,
        held,
        &[DownloadState::Resolving, DownloadState::Downloading],
    )
    .await;
    database
        .defer_par2_verdict(held, 1)
        .await
        .expect("verdict held");
    drive(&database, held, &[DownloadState::Verifying]).await;
    assert_eq!(
        state_of(&database, postponed).await.state,
        DownloadState::Skipped
    );
    let mut events = database.subscribe();

    database
        .fail_hopeless_usenet_package(package, hopeless())
        .await
        .expect("given up");

    for id in [paused, held, queued] {
        let row = state_of(&database, id).await;
        assert_eq!(row.state, DownloadState::Failed, "{}", row.file_name);
        let failure = row.last_error.expect("failure recorded");
        assert_eq!(failure.code.as_deref(), Some("usenet.job_hopeless"));
        assert_eq!(
            failure.params.get("available_blocks").map(String::as_str),
            Some("1")
        );
    }
    assert_eq!(
        state_of(&database, running).await.state,
        DownloadState::Downloading,
        "a running file is its runner's to stop"
    );
    assert_eq!(
        state_of(&database, postponed).await.state,
        DownloadState::Skipped,
        "a postponed volume stays postponed"
    );
    let mut announced = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::UsenetChanged {
            assert_eq!(event.payload["state"], "hopeless");
            assert_eq!(event.payload["missing_blocks"], "9");
            announced = true;
        }
    }
    assert!(announced, "the set given up is announced once");
    assert_eq!(
        package_state(&database, package).await,
        rd_core::PackageState::Downloading,
        "one file still runs"
    );

    // The runner's file ends with the same verdict, and the package reads as failed.
    database
        .record_failure(running, hopeless(), None)
        .await
        .expect("runner's failure");
    assert_eq!(
        package_state(&database, package).await,
        rd_core::PackageState::Failed
    );

    // A retry by hand puts the package back into the queue.
    database
        .transition_download(paused, DownloadState::Queued)
        .await
        .expect("retried");
    assert_eq!(
        package_state(&database, package).await,
        rd_core::PackageState::Queued
    );
}

/// The derivation is only for a set given up: a package with an ordinary failed file still
/// reads as queued, as it always did.
#[tokio::test]
async fn an_ordinary_failure_does_not_make_the_package_fail() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, package, rows) = release(directory.path()).await;
    for name in NAMES.iter().filter(|name| !name.contains(".vol")) {
        let id = id_of(&rows, name);
        drive(
            &database,
            id,
            &[DownloadState::Resolving, DownloadState::Downloading],
        )
        .await;
        database
            .record_failure(
                id,
                Failure::coded(
                    FailureKind::Permanent,
                    "usenet.all_segments_missing",
                    "All segments of the file are missing on every server",
                ),
                None,
            )
            .await
            .expect("failed");
    }
    assert_eq!(
        package_state(&database, package).await,
        rd_core::PackageState::Queued
    );
}

/// The abort can come while post-processing waits for postponed volumes it re-queued
/// (RD-107-04): the PAR2 step waiting for them ends with the set, in the same write, instead of
/// waiting under a failed package; a step that finished stays as it was.
#[tokio::test]
async fn a_par2_step_waiting_for_its_volumes_ends_with_the_set() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, package, rows) = release(directory.path()).await;
    let owner = package.to_string();
    let volume = id_of(&rows, "release.vol000+01.par2");
    // Where the refill leaves the package: the volume back in the queue, the step waiting.
    database
        .transition_download(volume, DownloadState::Queued)
        .await
        .expect("volume released");
    database
        .checkpoint_postprocess_coded(
            owner.clone(),
            rd_core::PostprocessKind::Par2,
            "/out/release.par2".to_owned(),
            rd_core::PostprocessState::Queued,
            None,
            Some("waiting for 1 postponed PAR2 volume(s) carrying 1 block(s)".to_owned()),
            Some("postprocess.par2_awaiting_blocks".to_owned()),
            [("volumes".to_owned(), "1".to_owned())]
                .into_iter()
                .collect(),
        )
        .await
        .expect("awaiting step");
    database
        .checkpoint_postprocess(
            owner.clone(),
            rd_core::PostprocessKind::Par2,
            "/out/other.par2".to_owned(),
            rd_core::PostprocessState::Completed,
            None,
            None,
        )
        .await
        .expect("finished step");

    database
        .fail_hopeless_usenet_package(package, hopeless())
        .await
        .expect("given up");

    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let waiting = steps
        .iter()
        .find(|step| step.source_path == "/out/release.par2")
        .expect("awaiting step");
    assert_eq!(waiting.state, rd_core::PostprocessState::Failed);
    assert_eq!(waiting.code.as_deref(), Some("usenet.job_hopeless"));
    assert_eq!(
        waiting.params.get("missing_blocks").map(String::as_str),
        Some("9")
    );
    let finished = steps
        .iter()
        .find(|step| step.source_path == "/out/other.par2")
        .expect("finished step");
    assert_eq!(finished.state, rd_core::PostprocessState::Completed);
    assert_eq!(
        state_of(&database, volume).await.state,
        DownloadState::Failed,
        "the re-queued volume is not fetched any more"
    );
}

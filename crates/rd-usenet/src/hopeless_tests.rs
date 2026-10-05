//! RD-1100-02: a Usenet set that can no longer be repaired is given up as soon as that is
//! certain, and only then.

use std::time::Duration;

use rd_core::{
    DownloadFile, DownloadPriority, DownloadState, ImportMode, NzbImportId, NzbSegmentState,
    PackageId, PackageState,
};
use rd_db::{Database, NewNzbFile, NewNzbImport, NewNzbSegment};

use crate::worker_tests::{
    server, single_part_article, spawn_scripted_fixture, start_scheduler_with, storage_root_at,
    wait_for_files, yenc_encode,
};

/// One article of a file `name` of four bytes in two articles.
fn half(name: &str, number: u64, payload: &[u8]) -> Vec<u8> {
    let begin = (number - 1) * 2 + 1;
    let end = begin + payload.len() as u64 - 1;
    let mut article = format!(
        "222 body follows\r\n=ybegin part={number} line=128 size=4 name={name}\r\n=ypart begin={begin} end={end}\r\n"
    )
    .into_bytes();
    article.extend(yenc_encode(payload));
    article.extend(
        format!(
            "\r\n=yend size={} part={number} pcrc32={:08x}\r\n.\r\n",
            payload.len(),
            crc32fast::hash(payload)
        )
        .as_bytes(),
    );
    article
}

fn segment(number: u32, bytes: u64, message_id: &str) -> NewNzbSegment {
    NewNzbSegment {
        number,
        bytes,
        message_id: message_id.to_owned(),
    }
}

fn nzb_file(subject: &str, segments: Vec<NewNzbSegment>) -> NewNzbFile {
    NewNzbFile {
        subject: subject.to_owned(),
        poster: "fixture".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments,
    }
}

/// Two archive volumes, the PAR2 index and one recovery volume named `volume`.
///
/// The first archive's first article weighs 10 000 bytes in the NZB, the volume `volume_bytes`;
/// the bytes the servers send are only what the assembly needs. The arithmetic reads the NZB.
fn release(digest: &str, volume: &str, volume_bytes: u64) -> NewNzbImport {
    NewNzbImport {
        name: "release.nzb".to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        source: rd_core::IngressSource::Manual,
        priority: None,
        import_mode: ImportMode::Enqueue,
        source_path: None,
        password: None,
        announce_arrival: true,
        files: vec![
            nzb_file(
                "release.part1.rar",
                vec![
                    segment(1, 10_000, "p1a@example.test"),
                    segment(2, 2, "p1b@example.test"),
                ],
            ),
            nzb_file("release.part2.rar", vec![segment(1, 2, "p2@example.test")]),
            nzb_file("release.par2", vec![segment(1, 64, "index@example.test")]),
            nzb_file(
                volume,
                vec![segment(1, volume_bytes, "volume@example.test")],
            ),
        ],
    }
}

/// What a server answers for the set; `first` is the first archive's first article, `None`
/// being a `430`.
fn articles(first: Option<Vec<u8>>) -> Vec<(String, Option<Vec<u8>>)> {
    let mut index = b"PAR2\0PKT".to_vec();
    index.extend(std::iter::repeat_n(0_u8, 56));
    vec![
        ("p1a@example.test".to_owned(), first),
        (
            "p1b@example.test".to_owned(),
            Some(half("release.part1.rar", 2, b"cd")),
        ),
        (
            "p2@example.test".to_owned(),
            Some(single_part_article("release.part2.rar", b"ef")),
        ),
        (
            "index@example.test".to_owned(),
            Some(single_part_article("release.par2", &index)),
        ),
        (
            "volume@example.test".to_owned(),
            Some(single_part_article("release.vol000+01.par2", &index)),
        ),
    ]
}

struct Queued {
    directory: tempfile::TempDir,
    database: Database,
    package: PackageId,
    import: NzbImportId,
}

/// The release queued as one package, under `settings`.
async fn queued(
    digest: &str,
    volume: &str,
    volume_bytes: u64,
    settings: serde_json::Value,
) -> Queued {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("hopeless.sqlite"))
        .await
        .expect("database");
    let output = directory.path().join("output");
    database
        .create_storage_root(rd_core::StorageRootId::new(), storage_root_at(&output))
        .await
        .expect("storage root");
    database
        .set_setting("service.settings".to_owned(), settings)
        .await
        .expect("settings");
    let import = database
        .add_nzb_import(release(digest, volume, volume_bytes))
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(import.id, output, DownloadPriority::Normal, false)
        .await
        .expect("enqueue");
    Queued {
        directory,
        database,
        package: package.id,
        import: import.id,
    }
}

fn row<'a>(files: &'a [DownloadFile], name: &str) -> &'a DownloadFile {
    files
        .iter()
        .find(|file| file.file_name == name)
        .unwrap_or_else(|| panic!("no row for {name}"))
}

fn code(file: &DownloadFile) -> Option<&str> {
    file.last_error
        .as_ref()
        .and_then(|failure| failure.code.as_deref())
}

async fn package_state(database: &Database, package: PackageId) -> PackageState {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = database
                .get_package(package)
                .await
                .expect("package")
                .expect("package exists")
                .state;
            if state == PackageState::Failed {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the package reads as failed")
}

/// The acceptance case: the set has lost more than it can repair, so it ends after the file
/// that showed it, with the code and both counts - and the rest is never fetched.
#[tokio::test]
async fn a_set_with_more_missing_blocks_than_its_volumes_hold_is_given_up_early() {
    let set = queued("b1", "release.vol000+01.par2", 1_000, serde_json::json!({})).await;
    let address = spawn_scripted_fixture(articles(None)).await;
    set.database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    // One file at a time, so the damaged archive really is the first to finish.
    let scheduler = start_scheduler_with(set.directory.path(), &set.database, 1).await;
    let files = wait_for_files(
        &set.database,
        set.package,
        &[DownloadState::Failed, DownloadState::Skipped],
    )
    .await;

    let first = row(&files, "release.part1.rar");
    let failure = first.last_error.clone().expect("failure recorded");
    assert_eq!(failure.code.as_deref(), Some("usenet.job_hopeless"));
    assert_eq!(
        failure.params.get("missing_blocks").map(String::as_str),
        Some("9")
    );
    assert_eq!(
        failure.params.get("available_blocks").map(String::as_str),
        Some("1")
    );
    for name in ["release.part2.rar", "release.par2"] {
        assert_eq!(row(&files, name).state, DownloadState::Failed, "{name}");
        assert_eq!(
            code(row(&files, name)),
            Some("usenet.job_hopeless"),
            "{name}"
        );
    }
    assert_eq!(
        row(&files, "release.vol000+01.par2").state,
        DownloadState::Skipped,
        "a postponed volume stays postponed"
    );

    let nzb_files = set
        .database
        .list_nzb_files(set.import)
        .await
        .expect("files");
    for nzb in nzb_files
        .iter()
        .filter(|nzb| nzb.subject != "release.part1.rar")
    {
        assert!(
            nzb.segments.iter().all(|segment| {
                segment.state == NzbSegmentState::Queued && segment.server_attempts == 0
            }),
            "{} was asked for after the set was given up",
            nzb.subject
        );
    }
    let assembled = nzb_files
        .iter()
        .find(|nzb| nzb.subject == "release.part1.rar")
        .and_then(|nzb| nzb.output_path.clone())
        .expect("output path");
    assert!(
        std::path::Path::new(&assembled).is_file(),
        "what was downloaded stays"
    );
    assert_eq!(
        package_state(&set.database, set.package).await,
        PackageState::Failed
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// Switched off, the same set downloads to the end and the holes wait for the PAR2 stage.
#[tokio::test]
async fn switched_off_the_same_set_downloads_everything_as_before() {
    let set = queued(
        "b2",
        "release.vol000+01.par2",
        1_000,
        serde_json::json!({ "fail_hopeless_jobs": false }),
    )
    .await;
    let address = spawn_scripted_fixture(articles(None)).await;
    set.database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let scheduler = start_scheduler_with(set.directory.path(), &set.database, 1).await;
    let files = wait_for_files(
        &set.database,
        set.package,
        &[DownloadState::Completed, DownloadState::Skipped],
    )
    .await;
    assert!(
        files
            .iter()
            .all(|file| code(file) != Some("usenet.job_hopeless")),
        "{files:?}"
    );
    assert_eq!(
        row(&files, "release.part2.rar").state,
        DownloadState::Completed
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// A gap the recovery volume covers is left to the repair: the rest of the set comes down.
#[tokio::test]
async fn a_repairable_set_keeps_downloading() {
    let set = queued(
        "b3",
        "release.vol000+20.par2",
        20_000,
        serde_json::json!({}),
    )
    .await;
    let address = spawn_scripted_fixture(articles(None)).await;
    set.database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let scheduler = start_scheduler_with(set.directory.path(), &set.database, 1).await;
    let files = wait_for_files(
        &set.database,
        set.package,
        &[DownloadState::Completed, DownloadState::Skipped],
    )
    .await;
    assert!(
        files
            .iter()
            .all(|file| code(file) != Some("usenet.job_hopeless")),
        "{files:?}"
    );
    assert_eq!(
        row(&files, "release.part1.rar").state,
        DownloadState::Completed,
        "the hole goes to repair"
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// An article only the first server lacks is not missing: the backup delivers it, and the same
/// set that is given up without it downloads whole.
#[tokio::test]
async fn an_article_missing_only_on_the_first_server_does_not_count() {
    let set = queued("b4", "release.vol000+01.par2", 1_000, serde_json::json!({})).await;
    let primary = spawn_scripted_fixture(articles(None)).await;
    let backup = spawn_scripted_fixture(articles(Some(half("release.part1.rar", 1, b"ab")))).await;
    set.database
        .create_usenet_server(server("Primary", primary, 0))
        .await
        .expect("primary NNTP server");
    set.database
        .create_usenet_server(server("Backup", backup, 1))
        .await
        .expect("backup NNTP server");
    let scheduler = start_scheduler_with(set.directory.path(), &set.database, 1).await;
    let files = wait_for_files(
        &set.database,
        set.package,
        &[DownloadState::Completed, DownloadState::Skipped],
    )
    .await;
    assert!(
        files
            .iter()
            .all(|file| code(file) != Some("usenet.job_hopeless")),
        "{files:?}"
    );
    let first = set
        .database
        .list_nzb_files(set.import)
        .await
        .expect("files")
        .into_iter()
        .find(|nzb| nzb.subject == "release.part1.rar")
        .expect("first archive");
    assert!(
        first
            .segments
            .iter()
            .all(|segment| segment.state == NzbSegmentState::Completed),
        "the backup delivered what the primary refused"
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// Crash point `usenet.before_hopeless_abort`: the verdict persists nothing, so a stop before
/// its write fails no row, and the judgement after the restart reads the same records and
/// reaches the same counts and the same rows.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_stop_between_the_verdict_and_its_write_gives_the_same_verdict_after_the_restart() {
    use rd_core::failpoint::FailpointGuard;

    use crate::hopeless::{Ending, Verdicts, give_up, judge};

    let set = queued("b5", "release.vol000+01.par2", 1_000, serde_json::json!({})).await;
    let rows = set
        .database
        .downloads_for_package(set.package)
        .await
        .expect("rows");
    let first = row(&rows, "release.part1.rar").clone();
    let refused = set
        .database
        .list_nzb_files(set.import)
        .await
        .expect("files")
        .into_iter()
        .find(|nzb| Some(nzb.id) == first.nzb_file_id)
        .and_then(|nzb| nzb.segments.into_iter().find(|segment| segment.number == 1))
        .expect("first article");
    // Where the runner leaves the first archive: its first article refused by every server,
    // the file assembled with the hole and its verdict held open.
    let ends_with_a_hole = || async {
        set.database
            .set_nzb_segment_state(refused.id, NzbSegmentState::Failed, None)
            .await
            .expect("refused article");
        for state in [DownloadState::Resolving, DownloadState::Downloading] {
            set.database
                .transition_download(first.id, state)
                .await
                .expect("on its way");
        }
        set.database
            .defer_par2_verdict(first.id, 1)
            .await
            .expect("verdict held");
    };
    ends_with_a_hole().await;

    let guard = FailpointGuard::after("usenet.before_hopeless_abort", 0);
    let verdict = judge(
        &set.database,
        set.package,
        set.import,
        first.id,
        Ending::Holes,
    )
    .await
    .expect("judged")
    .expect("beyond repair");
    let error = give_up(
        &set.database,
        &Verdicts::default(),
        set.package,
        set.import,
        &verdict,
    )
    .await
    .expect_err("the process stops before the write");
    assert!(guard.fired(), "the crash point was never reached: {error}");
    drop(guard);
    let after_stop = set
        .database
        .downloads_for_package(set.package)
        .await
        .expect("rows");
    assert!(
        after_stop
            .iter()
            .all(|file| file.state != DownloadState::Failed),
        "no row failed before the write: {after_stop:?}"
    );

    // The restart: the interrupted row goes back to the queue, its file is fetched again and
    // its first article refused again.
    set.database
        .recover_interrupted()
        .await
        .expect("crash recovery");
    ends_with_a_hole().await;
    let again = judge(
        &set.database,
        set.package,
        set.import,
        first.id,
        Ending::Holes,
    )
    .await
    .expect("judged")
    .expect("still beyond repair");
    assert_eq!(again.code, verdict.code);
    assert_eq!(again.params, verdict.params);
    give_up(
        &set.database,
        &Verdicts::default(),
        set.package,
        set.import,
        &again,
    )
    .await
    .expect("written");
    let after_restart = set
        .database
        .downloads_for_package(set.package)
        .await
        .expect("rows");
    for name in ["release.part2.rar", "release.par2"] {
        assert_eq!(
            row(&after_restart, name).state,
            DownloadState::Failed,
            "{name}"
        );
        assert_eq!(
            code(row(&after_restart, name)),
            Some("usenet.job_hopeless"),
            "{name}"
        );
    }
    assert_eq!(
        row(&after_restart, "release.vol000+01.par2").state,
        DownloadState::Skipped
    );
}

//! NZB imports in the queue: paused enqueue, history, segments, assembly and recovery.

use rd_core::{
    ImportMode, IngressSource, NzbSegmentState, PackageId, PostprocessKind, PostprocessState,
};

use crate::{Database, NewNzbFile, NewNzbImport, NewNzbSegment};

/// RD-107-09: "add paused" has to reach the rows an NZB import creates.
///
/// Before this, `enqueue_import` wrote the literal `'queued'` for every download row, so the
/// scheduler picked a "paused" NZB up straight away. The package row itself stays `queued` —
/// that is what the collector path does too, and the dispatcher keys off the download state.
#[tokio::test]
async fn enqueuing_an_nzb_import_paused_creates_paused_download_rows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("paused.sqlite"))
        .await
        .expect("database");

    let import = |name: &str, digest: &str, message: &str| NewNzbImport {
        name: name.to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        priority: None,
        import_mode: ImportMode::Enqueue,
        source: IngressSource::Manual,
        source_path: None,
        password: None,
        announce_arrival: false,
        files: vec![NewNzbFile {
            subject: format!("{name}.bin"),
            poster: "poster".to_owned(),
            groups: vec!["alt.binaries.test".to_owned()],
            segments: vec![NewNzbSegment {
                number: 1,
                bytes: 128,
                message_id: format!("{message}@example.test"),
            }],
        }],
    };

    let paused_import = database
        .add_nzb_import(import("paused.nzb", "a1", "paused-1"))
        .await
        .expect("import");
    let paused_package = database
        .enqueue_nzb_import(
            paused_import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            true,
        )
        .await
        .expect("enqueue paused");

    let started_import = database
        .add_nzb_import(import("started.nzb", "b2", "started-1"))
        .await
        .expect("import");
    let started_package = database
        .enqueue_nzb_import(
            started_import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue started");

    let downloads = database.list_downloads().await.expect("downloads");
    let states = |package: PackageId| {
        downloads
            .iter()
            .filter(|file| file.package_id == package)
            .map(|file| file.state)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        states(paused_package.id),
        vec![rd_core::DownloadState::Paused],
        "an NZB enqueued paused must not be dispatchable"
    );
    assert_eq!(
        states(started_package.id),
        vec![rd_core::DownloadState::Queued],
        "the ordinary path still starts immediately"
    );
    assert_eq!(
        paused_package.state,
        rd_core::PackageState::Queued,
        "the package row stays queued, exactly as the collector path leaves it"
    );
}

#[tokio::test]
async fn forgetting_import_history_removes_the_import_but_keeps_the_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("forget.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "history.nzb".to_owned(),
            sha256: "ef".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "history.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "history-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    database
        .forget_nzb_import_history(package.id)
        .await
        .expect("forget history");

    assert!(
        database
            .list_nzb_imports()
            .await
            .expect("imports")
            .is_empty()
    );
    assert!(
        database
            .list_packages()
            .await
            .expect("packages")
            .iter()
            .any(|candidate| candidate.id == package.id)
    );
    // Without an import link the call is a no-op instead of an error.
    database
        .forget_nzb_import_history(package.id)
        .await
        .expect("idempotent");
}

#[tokio::test]
async fn nzb_segment_attempt_and_crc_are_persistent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("segments.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "segments.nzb".to_owned(),
            sha256: "cd".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "file.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "part-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let files = database.list_nzb_files(import.id).await.expect("files");
    let segment_id = files[0].segments[0].id;
    database
        .set_nzb_segment_state(segment_id, NzbSegmentState::Downloading, None)
        .await
        .expect("attempt");
    database
        .set_nzb_segment_state(segment_id, NzbSegmentState::Completed, Some(0x1234_abcd))
        .await
        .expect("completion");

    let files = database.list_nzb_files(import.id).await.expect("files");
    let segment = &files[0].segments[0];
    assert_eq!(segment.state, NzbSegmentState::Completed);
    assert_eq!(segment.server_attempts, 1);
    assert_eq!(segment.crc32.as_deref(), Some("1234abcd"));
}

/// RD-130-22: a batch of assembly checkpoints is one transaction - every article in it is
/// confirmed, with its attempts, or none is.
#[tokio::test]
async fn nzb_assembly_batch_confirms_every_segment_or_none() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("batch.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "batch.nzb".to_owned(),
            sha256: "ce".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "file.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: (1..=3)
                    .map(|number| NewNzbSegment {
                        number,
                        bytes: 100,
                        message_id: format!("part-{number}@example.test"),
                    })
                    .collect(),
            }],
        })
        .await
        .expect("import");
    let file = database.list_nzb_files(import.id).await.expect("files")[0].clone();
    let mut segments = file.segments.clone();
    segments.sort_by_key(|segment| segment.number);
    let ids: Vec<_> = segments.iter().map(|segment| segment.id).collect();
    let range = |segment_id, number: u64, attempts| crate::AssembledSegment {
        segment_id,
        part_begin: (number - 1) * 100 + 1,
        part_end: number * 100,
        crc32: 0x0000_1000 + u32::try_from(number).expect("small"),
        attempts,
    };

    // One segment the file does not have spoils the whole batch.
    let refused = database
        .checkpoint_nzb_assembly_segments(
            file.id,
            "file.bin".to_owned(),
            300,
            vec![
                range(ids[0], 1, 1),
                range(rd_core::NzbSegmentId::new(), 2, 1),
            ],
        )
        .await;
    assert!(
        refused.is_err(),
        "a batch naming a foreign segment is refused"
    );
    let untouched = database.list_nzb_files(import.id).await.expect("files");
    assert!(
        untouched[0]
            .segments
            .iter()
            .all(|segment| segment.state != NzbSegmentState::Completed),
        "a refused batch confirms nothing"
    );

    database
        .checkpoint_nzb_assembly_segments(
            file.id,
            "file.bin".to_owned(),
            300,
            vec![range(ids[0], 1, 1), range(ids[2], 3, 2)],
        )
        .await
        .expect("batch");
    let files = database.list_nzb_files(import.id).await.expect("files");
    let by_number = |number| {
        files[0]
            .segments
            .iter()
            .find(|segment| segment.number == number)
            .expect("segment")
    };
    assert_eq!(by_number(1).state, NzbSegmentState::Completed);
    assert_eq!(by_number(1).server_attempts, 1);
    assert_eq!(by_number(1).part_begin.map(|value| value.get()), Some(1));
    assert_eq!(by_number(2).state, NzbSegmentState::Queued);
    assert_eq!(by_number(3).state, NzbSegmentState::Completed);
    assert_eq!(
        by_number(3).server_attempts,
        2,
        "a backup attempt is counted"
    );
    assert_eq!(by_number(3).crc32.as_deref(), Some("00001003"));
    assert_eq!(files[0].assembly_name.as_deref(), Some("file.bin"));
}

#[tokio::test]
async fn usenet_file_and_postprocess_checkpoints_survive_recovery() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("checkpoints.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "checkpoint.nzb".to_owned(),
            sha256: "fe".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "archive.zip".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 42,
                    message_id: "checkpoint@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    database
        .checkpoint_nzb_file_output(file.id, "/downloads/archive.zip".to_owned())
        .await
        .expect("file checkpoint");
    database
        .checkpoint_postprocess(
            import.id.to_string(),
            PostprocessKind::ExtractZip,
            "/downloads/archive.zip".to_owned(),
            PostprocessState::Running,
            Some("/downloads/archive".to_owned()),
            None,
        )
        .await
        .expect("postprocess checkpoint");
    database.recover_interrupted().await.expect("recovery");

    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    assert_eq!(file.output_path.as_deref(), Some("/downloads/archive.zip"));
    let steps = database
        .list_postprocess_steps(&import.id.to_string())
        .await
        .expect("steps");
    assert_eq!(steps[0].state, PostprocessState::Queued);
    assert_eq!(steps[0].output_path.as_deref(), Some("/downloads/archive"));
}

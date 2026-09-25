use rd_core::{ImportMode, NzbSegmentState};
use rd_db::{Database, NewNzbFile, NewNzbImport, NewNzbSegment};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::assembly_resume::{prepare, validates_completed_file};

#[tokio::test]
async fn resumes_only_crc_verified_contiguous_synced_ranges() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("resume.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "resume.nzb".to_owned(),
            sha256: "aa".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "file.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![
                    NewNzbSegment {
                        number: 1,
                        bytes: 2,
                        message_id: "one@example.test".to_owned(),
                    },
                    NewNzbSegment {
                        number: 2,
                        bytes: 2,
                        message_id: "two@example.test".to_owned(),
                    },
                ],
            }],
        })
        .await
        .expect("import");
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    let part_path = directory.path().join("file.part");
    let mut part = tokio::fs::File::create(&part_path)
        .await
        .expect("part file");
    part.write_all(&[1, 2]).await.expect("confirmed bytes");
    part.sync_data().await.expect("sync confirmed bytes");
    database
        .checkpoint_nzb_assembly_segment(
            file.id,
            file.segments[0].id,
            "file.bin".to_owned(),
            4,
            1,
            2,
            crc32fast::hash(&[1, 2]),
        )
        .await
        .expect("segment checkpoint");
    part.write_all(&[9, 9]).await.expect("unconfirmed tail");
    drop(part);

    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    let resume = prepare(&database, file, &part_path)
        .await
        .expect("resume state");
    assert_eq!(resume.written, [(1, 2)]);
    assert_eq!(resume.name.as_deref(), Some("file.bin"));
    assert_eq!(resume.declared_size, Some(4));
    assert_eq!(resume.remaining.len(), 1);
    // The unconfirmed tail is left where it is (RD-108-26): the article that owns those bytes
    // writes over them when it arrives, and if it never arrives they are zero-filled when the
    // file is finished. Truncating would only have moved the same work earlier.
    assert_eq!(resume.output.metadata().await.expect("metadata").len(), 4);
    drop(resume.output);

    let complete_path = directory.path().join("complete.bin");
    tokio::fs::write(&complete_path, [1, 2, 3, 4])
        .await
        .expect("complete file");
    database
        .checkpoint_nzb_assembly_segment(
            file.id,
            file.segments[1].id,
            "file.bin".to_owned(),
            4,
            3,
            4,
            crc32fast::hash(&[3, 4]),
        )
        .await
        .expect("second segment checkpoint");
    let complete = &database.list_nzb_files(import.id).await.expect("files")[0];
    assert!(
        validates_completed_file(complete, &complete_path)
            .await
            .expect("validate completed file")
    );
    tokio::fs::write(&complete_path, [1, 2, 3, 9])
        .await
        .expect("corrupt completed file");
    assert!(
        !validates_completed_file(complete, &complete_path)
            .await
            .expect("reject completed file")
    );

    let mut corrupt = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&part_path)
        .await
        .expect("open part");
    corrupt
        .seek(std::io::SeekFrom::Start(0))
        .await
        .expect("seek");
    corrupt.write_all(&[7]).await.expect("corrupt range");
    corrupt.sync_data().await.expect("sync corruption");
    drop(corrupt);
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    let reset = prepare(&database, file, &part_path)
        .await
        .expect("reset state");
    assert!(reset.written.is_empty());
    assert_eq!(reset.remaining.len(), 2);
    drop(reset.output);
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    assert_eq!(file.segments[0].state, NzbSegmentState::Queued);
}

/// A file with a hole resumes as a file with a hole (RD-108-26).
///
/// Articles are written where they belong now, so the second segment can be on disk while the
/// first is still missing. The old resume walked from the front and stopped at the first gap,
/// which would throw the proven second segment away and fetch it again.
#[tokio::test]
async fn a_proven_range_behind_a_gap_is_kept() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("hole.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "hole.nzb".to_owned(),
            sha256: "bb".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "file.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![
                    NewNzbSegment {
                        number: 1,
                        bytes: 2,
                        message_id: "one@example.test".to_owned(),
                    },
                    NewNzbSegment {
                        number: 2,
                        bytes: 2,
                        message_id: "two@example.test".to_owned(),
                    },
                ],
            }],
        })
        .await
        .expect("import");
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    let part_path = directory.path().join("hole.part");
    // Byte 1 and 2 were never written; 3 and 4 are the second article, where it belongs.
    tokio::fs::write(&part_path, [0, 0, 3, 4])
        .await
        .expect("part file with a hole");
    database
        .checkpoint_nzb_assembly_segment(
            file.id,
            file.segments[1].id,
            "file.bin".to_owned(),
            4,
            3,
            4,
            crc32fast::hash(&[3, 4]),
        )
        .await
        .expect("second segment checkpoint");

    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    let resume = prepare(&database, file, &part_path)
        .await
        .expect("resume state");
    assert_eq!(resume.written, [(3, 4)]);
    assert_eq!(
        resume
            .remaining
            .iter()
            .map(|segment| segment.number)
            .collect::<Vec<_>>(),
        [1],
        "only the article that is not on disk is fetched again"
    );
    assert_eq!(resume.output.metadata().await.expect("metadata").len(), 4);
}

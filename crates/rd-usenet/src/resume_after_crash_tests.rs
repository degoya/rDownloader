//! The state the removed per-article `sync_data()` used to guard against (RD-108-25): the
//! database says a segment is on disk and the disk disagrees, because the power went before
//! the kernel flushed. The resume has to notice, keep what it can prove, and fetch the rest.

use std::{collections::HashMap, sync::Arc};

use rd_core::NzbSegmentState;
use rd_db::Database;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    test_support::{
        FixtureTiming, import_single_file, multipart_article, payload, reload, run_limits,
        spawn_fixture,
    },
    worker::{FileOutcome, download_file},
};

const SEGMENT_BYTES: usize = 3000;

/// A `.part` file shorter than the checkpoint claims loses the unflushed segments, never the
/// file: the resume keeps every CRC-proven range and fetches the rest again, writing it back
/// where it belongs (RD-108-26).
#[tokio::test]
async fn a_part_file_shorter_than_its_checkpoint_is_completed_from_its_proven_ranges() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("resume.sqlite"))
        .await
        .expect("database");
    let parts: Vec<Vec<u8>> = (0..3)
        .map(|index| payload(SEGMENT_BYTES, index * 13))
        .collect();
    let total = (3 * SEGMENT_BYTES) as u64;
    let mut articles = HashMap::new();
    let mut segments = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let message_id = format!("part-{}@example.test", index + 1);
        let begin = (index * SEGMENT_BYTES) as u64 + 1;
        articles.insert(
            message_id.clone(),
            Some(multipart_article(
                "file.bin",
                (index + 1) as u64,
                total,
                begin,
                part,
            )),
        );
        segments.push((message_id, SEGMENT_BYTES as u64));
    }
    let file = import_single_file(&database, "file.bin", &segments).await;
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");

    // The crash: segments 1 and 2 are checkpointed, but only segment 1 and half of segment 2
    // ever reached the disk.
    let part_path = staging.join(format!("{}.part", file.id));
    let mut part = tokio::fs::File::create(&part_path)
        .await
        .expect("part file");
    part.write_all(&parts[0]).await.expect("segment 1");
    part.write_all(&parts[1][..SEGMENT_BYTES / 2])
        .await
        .expect("half of segment 2");
    drop(part);
    for (index, segment) in file.segments.iter().take(2).enumerate() {
        let begin = (index * SEGMENT_BYTES) as u64 + 1;
        database
            .checkpoint_nzb_assembly_segment(
                file.id,
                segment.id,
                "file.bin".to_owned(),
                total,
                begin,
                begin + SEGMENT_BYTES as u64 - 1,
                crc32fast::hash(&parts[index]),
            )
            .await
            .expect("checkpoint");
    }
    let file = reload(&database, &file).await;
    assert!(
        file.segments
            .iter()
            .take(2)
            .all(|segment| segment.state == NzbSegmentState::Completed)
    );

    let (address, log) = spawn_fixture(Arc::new(articles), FixtureTiming::default()).await;
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 64 * 1024,
        max_connections: 2,
    }])
    .expect("pool");
    let outcome = download_file(
        &database,
        &pool,
        &CancellationToken::new(),
        &file,
        &staging,
        &destination,
        &run_limits(),
    )
    .await
    .expect("resumed download");
    let FileOutcome::Completed { path, missing } = outcome else {
        panic!("download was cancelled");
    };
    assert_eq!(missing, 0);

    // Only the unproven segments were fetched again; segment 1 was never asked for.
    let mut requested = log.requests();
    requested.sort();
    assert_eq!(requested, ["part-2@example.test", "part-3@example.test"]);
    let expected: Vec<u8> = parts.concat();
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(
        written, expected,
        "the assembled bytes differ from the article set"
    );
    assert!(
        !part_path.exists(),
        "the part file survived the completed download"
    );
    let file = reload(&database, &file).await;
    assert!(
        file.segments
            .iter()
            .all(|segment| segment.state == NzbSegmentState::Completed)
    );
    assert_eq!(
        file.segments[1].crc32.as_deref(),
        Some(format!("{:08x}", crc32fast::hash(&parts[1])).as_str()),
        "segment 2's checkpoint was written again from the fetched bytes"
    );
}

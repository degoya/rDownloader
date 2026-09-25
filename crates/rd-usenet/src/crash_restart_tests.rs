//! Crash and restart for NZB assembly - Axis A of the RD-140-04 recovery matrix, for the one
//! Usenet point registered in `rd_core::failpoint::CRASH_POINTS`.
//!
//! The instant is the one the removed per-article `sync_data()` sat next to (RD-108-25): an
//! article's bytes are appended to the `.part` file and the database has not recorded them.
//! A restart must not count them. The four invariants of `docs/recovery-matrix.md` are
//! asserted the way `rd-http`'s cases assert them: no confirmed byte invented, no confirmed
//! byte overwritten, the same bytes as an uninterrupted run, nothing left behind.

#![cfg(feature = "failpoints")]

use std::{collections::HashMap, sync::Arc};

use rd_core::{NzbSegmentState, failpoint::FailpointGuard};
use rd_db::Database;
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

#[tokio::test]
async fn an_article_written_but_not_checkpointed_is_fetched_again_after_the_crash() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("crash.sqlite"))
        .await
        .expect("database");
    let parts: Vec<Vec<u8>> = (0..3)
        .map(|index| payload(SEGMENT_BYTES, index * 17))
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
    let (address, log) = spawn_fixture(Arc::new(articles), FixtureTiming::default()).await;
    // One connection, so the fixture's request order is the assembly order.
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 64 * 1024,
        max_connections: 1,
    }])
    .expect("pool");
    let part_path = staging.join(format!("{}.part", file.id));

    // The crash: segment 1 is written and checkpointed, segment 2 is written and the
    // process stops before its checkpoint.
    let guard = FailpointGuard::after("usenet.after_article_write", 1);
    let error = match download_file(
        &database,
        &pool,
        &CancellationToken::new(),
        &file,
        &staging,
        &destination,
        &run_limits(),
    )
    .await
    {
        Ok(_) => panic!("the crash point did not stop the download"),
        Err(error) => error,
    };
    assert!(guard.fired(), "the crash point was never reached: {error}");
    drop(guard);
    let on_disk = tokio::fs::metadata(&part_path)
        .await
        .expect("part file")
        .len();
    assert_eq!(
        on_disk,
        (2 * SEGMENT_BYTES) as u64,
        "two articles reached the disk before the crash"
    );
    let crashed = reload(&database, &file).await;
    assert_eq!(crashed.segments[0].state, NzbSegmentState::Completed);
    assert_ne!(
        crashed.segments[1].state,
        NzbSegmentState::Completed,
        "invariant 1: the unrecorded article is not counted as confirmed"
    );

    // The restart.
    let outcome = download_file(
        &database,
        &pool,
        &CancellationToken::new(),
        &crashed,
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
    let requested = log.requests();
    assert_eq!(
        requested
            .iter()
            .filter(|id| id.as_str() == "part-1@example.test")
            .count(),
        1,
        "invariant 2: the confirmed segment was neither fetched nor written again"
    );
    assert_eq!(
        requested
            .iter()
            .filter(|id| id.as_str() == "part-2@example.test")
            .count(),
        2,
        "the unrecorded article was fetched again"
    );
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(
        written,
        parts.concat(),
        "invariant 3: the resumed file has the bytes of an uninterrupted run"
    );
    assert!(
        !part_path.exists(),
        "invariant 4: the part file was left behind"
    );
    let finished = reload(&database, &file).await;
    assert!(
        finished
            .segments
            .iter()
            .all(|segment| segment.state == NzbSegmentState::Completed)
    );
}

/// Every registered crash point for this crate has a case here.
///
/// Registering a point and never covering it is the failure this catches: the registry would
/// list an invariant nobody checks, which reads as coverage and is not.
#[test]
fn every_usenet_crash_point_is_exercised_by_a_case() {
    let source = include_str!("crash_restart_tests.rs");
    for point in rd_core::failpoint::CRASH_POINTS
        .iter()
        .filter(|point| point.owner == "rd-usenet")
    {
        assert!(
            source.contains(&format!("FailpointGuard::after(\"{}\"", point.name)),
            "{} is registered but no case in this file arms it",
            point.name
        );
    }
}

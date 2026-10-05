//! A yEnc header's `size=` sizes nothing the NZB does not account for, and the gaps of missing
//! articles are filled only where the disk can take them (RD-1101-16, audit S6).

use std::{collections::HashMap, sync::Arc};

use rd_core::{ByteCount, Failure, StorageSettings};
use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    bounds::declared_size_limit,
    test_support::{
        FixtureTiming, SEGMENT_BYTES, completed, import_single_file, multipart_article, payload,
        run_limits, spawn_fixture,
    },
    worker::{FileOutcome, download_file},
};

/// Three articles of [`SEGMENT_BYTES`], the second missing on the server, each announcing a
/// file of `announced` bytes; the NZB lists each at `listed` bytes.
fn set_with_a_hole(
    announced: u64,
    listed: u64,
) -> (crate::test_support::Articles, Vec<(String, u64)>) {
    let mut articles = HashMap::new();
    let mut segments = Vec::new();
    for number in 1..=3_u64 {
        let message_id = format!("part-{number}@example.test");
        let begin = (number - 1) * SEGMENT_BYTES as u64 + 1;
        let article = (number != 2).then(|| {
            multipart_article(
                "file.bin",
                number,
                announced,
                begin,
                &payload(SEGMENT_BYTES, number as usize),
            )
        });
        articles.insert(message_id.clone(), article);
        segments.push((message_id, listed));
    }
    (Arc::new(articles), segments)
}

struct Assembly {
    outcome: anyhow::Result<FileOutcome>,
    /// Held so the assembled file outlives `assemble` until the test has read it.
    _directory: tempfile::TempDir,
    /// The length of everything the assembly left in staging.
    staged: u64,
    /// How many files reached the destination.
    delivered: usize,
}

/// Assembles the set through the worker; `settings` goes into the service settings first.
async fn assemble(
    articles: crate::test_support::Articles,
    segments: &[(String, u64)],
    settings: Option<StorageSettings>,
) -> Assembly {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("bounds.sqlite"))
        .await
        .expect("database");
    if let Some(settings) = settings {
        database
            .set_setting(
                rd_db::SERVICE_SETTINGS_KEY.to_owned(),
                serde_json::to_value(settings).expect("settings"),
            )
            .await
            .expect("store settings");
    }
    let file = import_single_file(&database, "file.bin", segments).await;
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let (address, _log) = spawn_fixture(articles, FixtureTiming::default()).await;
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
    let outcome = download_file(
        &database,
        &pool,
        &CancellationToken::new(),
        &file,
        &staging,
        &destination,
        &run_limits(),
    )
    .await;
    let mut staged = 0;
    let mut entries = tokio::fs::read_dir(&staging).await.expect("read staging");
    while let Some(entry) = entries.next_entry().await.expect("entry") {
        staged += entry.metadata().await.expect("metadata").len();
    }
    let mut delivered = 0;
    let mut entries = tokio::fs::read_dir(&destination)
        .await
        .expect("read destination");
    while entries.next_entry().await.expect("entry").is_some() {
        delivered += 1;
    }
    Assembly {
        outcome,
        _directory: directory,
        staged,
        delivered,
    }
}

fn failure_code(outcome: anyhow::Result<FileOutcome>) -> Option<String> {
    match outcome {
        Ok(_) => None,
        Err(error) => error
            .downcast::<Failure>()
            .ok()
            .and_then(|failure| failure.code),
    }
}

#[test]
fn the_limit_leaves_room_for_rounding_and_nothing_like_a_terabyte() {
    assert_eq!(declared_size_limit(0), 1024 * 1024);
    assert_eq!(declared_size_limit(8_000), 8_000 + 1_000 + 1024 * 1024);
    assert_eq!(declared_size_limit(u64::MAX), u64::MAX);
    // The NZB of a 4 GiB file lists its articles encoded, a few per cent above the payload.
    let payload = 4_u64 << 30;
    assert!(payload <= declared_size_limit(payload + payload / 50));
    assert!(1_u64 << 40 > declared_size_limit(payload + payload / 50));
}

/// The finding: one article announcing far more than the NZB lists, with a hole to fill.
/// Before the bound the file was grown to that size and zero-filled.
#[tokio::test]
async fn a_header_announcing_more_than_the_nzb_lists_sizes_nothing() {
    let listed = SEGMENT_BYTES as u64;
    let announced = 8 * 1024 * 1024;
    assert!(announced > declared_size_limit(3 * listed));
    let (articles, segments) = set_with_a_hole(announced, listed);

    let assembly = assemble(articles, &segments, None).await;

    assert_eq!(
        failure_code(assembly.outcome).as_deref(),
        Some("usenet.declared_size_implausible")
    );
    assert_eq!(assembly.delivered, 0);
    assert!(
        assembly.staged <= 3 * listed,
        "{} bytes staged for a file the NZB lists at {}",
        assembly.staged,
        3 * listed
    );
}

/// The boundary the bound must not cross: an NZB that lists the articles encoded, a little
/// above what they decode to, still has its hole filled.
#[tokio::test]
async fn an_honest_header_still_has_its_hole_filled() {
    let announced = 3 * SEGMENT_BYTES as u64;
    let (articles, segments) = set_with_a_hole(announced, SEGMENT_BYTES as u64 + 60);

    let assembly = assemble(articles, &segments, None).await;

    let (path, missing) = completed(assembly.outcome);
    assert_eq!(missing, 1);
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(written.len() as u64, announced);
    assert!(
        written[SEGMENT_BYTES..2 * SEGMENT_BYTES]
            .iter()
            .all(|byte| *byte == 0)
    );
}

/// A hole is filled only where the disk keeps its reserve afterwards; here no disk could.
#[tokio::test]
async fn gaps_the_disk_cannot_take_beside_its_reserve_are_not_filled() {
    let announced = 3 * SEGMENT_BYTES as u64;
    let (articles, segments) = set_with_a_hole(announced, SEGMENT_BYTES as u64);
    let settings = StorageSettings {
        storage_minimum_free_bytes: ByteCount::new(1 << 60).expect("byte count"),
        ..StorageSettings::default()
    };

    let assembly = assemble(articles, &segments, Some(settings)).await;

    assert_eq!(
        failure_code(assembly.outcome).as_deref(),
        Some("usenet.gap_fill_no_space")
    );
    assert_eq!(assembly.delivered, 0);
}

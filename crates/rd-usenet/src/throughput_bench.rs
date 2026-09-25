//! The measurement RD-108-25 rests on: the same article set through the worker path, against
//! the scripted fixture with a round trip and a per-connection byte rate.
//!
//! No real provider is reachable where this runs, so the fixture stands in for one: every
//! `BODY` is answered `RTT` after it arrives and written at `RATE` on its connection, which
//! is the shape of a line whose latency and per-connection throughput are both bounded. The
//! number is only comparable with itself - the same command on the code before and after a
//! change - and that is what it is for. Ignored by default because it takes seconds and
//! prints rather than asserts:
//!
//! ```bash
//! cargo nextest run -p rd-usenet --run-ignored ignored-only -E 'test(fixture_throughput)' --no-capture
//! ```
//!
//! `RD_BENCH_RTT_MS` and `RD_BENCH_RATE_MIB` override the line: `RD_BENCH_RTT_MS=1
//! RD_BENCH_RATE_MIB=0` is a link so fast that the disk is what remains.

use std::{collections::HashMap, sync::Arc, time::Duration};

use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    test_support::{
        FixtureTiming, import_single_file, multipart_article, payload, run_limits, spawn_fixture,
    },
};

const CONNECTIONS: u16 = 10;
const ARTICLES: usize = 200;
const ARTICLE_BYTES: usize = 768 * 1024;
const DEFAULT_RTT_MS: u64 = 30;
const DEFAULT_RATE_MIB: u64 = 10;

fn setting(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "prints a throughput number; run on purpose, on a quiet machine"]
async fn fixture_throughput() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("bench.sqlite"))
        .await
        .expect("database");
    let total = (ARTICLES * ARTICLE_BYTES) as u64;
    let mut articles = HashMap::new();
    let mut segments = Vec::with_capacity(ARTICLES);
    let mut expected = Vec::with_capacity(ARTICLES * ARTICLE_BYTES);
    for index in 0..ARTICLES {
        let bytes = payload(ARTICLE_BYTES, index * 7);
        let message_id = format!("part-{}@bench.test", index + 1);
        let begin = (index * ARTICLE_BYTES) as u64 + 1;
        articles.insert(
            message_id.clone(),
            Some(multipart_article(
                "bench.bin",
                (index + 1) as u64,
                total,
                begin,
                &bytes,
            )),
        );
        segments.push((message_id, ARTICLE_BYTES as u64));
        expected.extend_from_slice(&bytes);
    }
    let rtt = Duration::from_millis(setting("RD_BENCH_RTT_MS", DEFAULT_RTT_MS));
    let rate_mib = setting("RD_BENCH_RATE_MIB", DEFAULT_RATE_MIB);
    let (address, log) = spawn_fixture(
        Arc::new(articles),
        FixtureTiming {
            rtt,
            bytes_per_second: (rate_mib > 0).then_some(rate_mib * 1024 * 1024),
        },
    )
    .await;
    let file = import_single_file(&database, "bench.bin", &segments).await;
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 4 * 1024 * 1024,
        max_connections: CONNECTIONS,
    }])
    .expect("pool");
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let shutdown = CancellationToken::new();
    let limits = run_limits();

    let started = std::time::Instant::now();
    let outcome = crate::worker::download_file(
        &database,
        &pool,
        &shutdown,
        &file,
        &staging,
        &destination,
        &limits,
    )
    .await
    .expect("download");
    let elapsed = started.elapsed();

    let crate::worker::FileOutcome::Completed { path, missing } = outcome else {
        panic!("download was cancelled");
    };
    assert_eq!(missing, 0);
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(written.len(), expected.len());
    assert_eq!(crc32fast::hash(&written), crc32fast::hash(&expected));
    let megabytes = total as f64 / 1_000_000.0;
    println!(
        "fixture_throughput: {ARTICLES} articles x {} KiB over {CONNECTIONS} connections, RTT {} ms, {} MiB/s per connection (0 = unpaced): {:.2} s, {:.1} MB/s ({} connections opened)",
        ARTICLE_BYTES / 1024,
        rtt.as_millis(),
        rate_mib,
        elapsed.as_secs_f64(),
        megabytes / elapsed.as_secs_f64(),
        log.connection_count(),
    );
}

const FILES: usize = 40;
const SMALL_ARTICLES: usize = 5;
const SMALL_ARTICLE_BYTES: usize = 192 * 1024;

/// What a release of many small files costs when the pool does not survive the file change.
///
/// The same forty files twice: once with a pool per file, which is what RD-108-25 shipped,
/// once with one pool for all of them (RD-108-26). Everything else is equal - the same
/// fixture, the same round trip, the same per-connection rate - so the difference is the
/// connection setup, and the connections opened say why.
///
/// ```bash
/// cargo nextest run -p rd-usenet --run-ignored ignored-only -E 'test(many_small_files)' --no-capture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "prints two throughput numbers; run on purpose, on a quiet machine"]
async fn many_small_files() {
    let rtt = Duration::from_millis(setting("RD_BENCH_RTT_MS", DEFAULT_RTT_MS));
    let rate_mib = setting("RD_BENCH_RATE_MIB", DEFAULT_RATE_MIB);
    for pool_per_file in [true, false] {
        let (elapsed, connections) = many_small_files_pass(rtt, rate_mib, pool_per_file).await;
        let megabytes = (FILES * SMALL_ARTICLES * SMALL_ARTICLE_BYTES) as f64 / 1_000_000.0;
        println!(
            "many_small_files ({}): {FILES} files x {SMALL_ARTICLES} articles over {CONNECTIONS} connections, RTT {} ms: {:.2} s, {:.1} MB/s ({connections} connections opened)",
            if pool_per_file {
                "a pool per file"
            } else {
                "one pool"
            },
            rtt.as_millis(),
            elapsed.as_secs_f64(),
            megabytes / elapsed.as_secs_f64(),
        );
    }
}

async fn many_small_files_pass(
    rtt: Duration,
    rate_mib: u64,
    pool_per_file: bool,
) -> (Duration, usize) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("small.sqlite"))
        .await
        .expect("database");
    let total = (SMALL_ARTICLES * SMALL_ARTICLE_BYTES) as u64;
    let mut articles = HashMap::new();
    let mut files = Vec::with_capacity(FILES);
    for file in 0..FILES {
        let mut segments = Vec::with_capacity(SMALL_ARTICLES);
        for index in 0..SMALL_ARTICLES {
            let message_id = format!("file-{file}-part-{index}@bench.test");
            let bytes = payload(SMALL_ARTICLE_BYTES, file * 31 + index);
            articles.insert(
                message_id.clone(),
                Some(multipart_article(
                    "small.bin",
                    (index + 1) as u64,
                    total,
                    (index * SMALL_ARTICLE_BYTES) as u64 + 1,
                    &bytes,
                )),
            );
            segments.push((message_id, SMALL_ARTICLE_BYTES as u64));
        }
        files.push(import_single_file(&database, &format!("small-{file}.bin"), &segments).await);
    }
    let (address, log) = spawn_fixture(
        Arc::new(articles),
        FixtureTiming {
            rtt,
            bytes_per_second: (rate_mib > 0).then_some(rate_mib * 1024 * 1024),
        },
    )
    .await;
    let server = NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 4 * 1024 * 1024,
        max_connections: CONNECTIONS,
    };
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let shutdown = CancellationToken::new();
    let limits = run_limits();
    let shared = NntpPool::new(vec![server.clone()]).expect("pool");

    let started = std::time::Instant::now();
    for file in &files {
        let pool = if pool_per_file {
            NntpPool::new(vec![server.clone()]).expect("pool")
        } else {
            shared.clone()
        };
        let outcome = crate::worker::download_file(
            &database,
            &pool,
            &shutdown,
            file,
            &staging,
            &destination,
            &limits,
        )
        .await
        .expect("download");
        assert!(matches!(
            outcome,
            crate::worker::FileOutcome::Completed { missing: 0, .. }
        ));
    }
    (started.elapsed(), log.connection_count())
}

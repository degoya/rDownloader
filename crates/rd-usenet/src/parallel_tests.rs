//! Several NZB files through one pool (RD-130-22): the connections move on to whatever is
//! still open when a file runs out of articles, and all files together never exceed a
//! server's connection limit.

use std::{collections::HashMap, sync::Arc, time::Duration};

use rd_core::NzbFileStatus;
use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    parallel::FileLoad,
    pool::PIPELINE_DEPTH,
    test_support::{
        Articles, FixtureTiming, import_single_file, multipart_article, payload, run_limits,
        spawn_fixture,
    },
    worker::{FileOutcome, download_file_counted},
};

const SEGMENT_BYTES: usize = 3000;

/// Scripts `sizes.len()` files of `sizes[i]` articles each and imports them.
async fn release(database: &Database, sizes: &[usize]) -> (Articles, Vec<NzbFileStatus>) {
    let mut articles = HashMap::new();
    let mut files = Vec::with_capacity(sizes.len());
    for (file, &count) in sizes.iter().enumerate() {
        let total = (count * SEGMENT_BYTES) as u64;
        let mut segments = Vec::with_capacity(count);
        for index in 0..count {
            let message_id = format!("file-{file}-part-{index}@example.test");
            articles.insert(
                message_id.clone(),
                Some(multipart_article(
                    &format!("file-{file}.bin"),
                    (index + 1) as u64,
                    total,
                    (index * SEGMENT_BYTES) as u64 + 1,
                    &payload(SEGMENT_BYTES, file * 31 + index),
                )),
            );
            segments.push((message_id, SEGMENT_BYTES as u64));
        }
        files.push(import_single_file(database, &format!("file-{file}.bin"), &segments).await);
    }
    (Arc::new(articles), files)
}

fn server(address: std::net::SocketAddr, connections: u16) -> NntpServerConfig {
    NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 64 * 1024,
        max_connections: connections,
    }
}

/// Runs every file at once through `pool`, the way the runner does with several slots.
async fn run_together(
    database: &Database,
    pool: &NntpPool,
    files: &[NzbFileStatus],
    directory: &std::path::Path,
) -> Arc<FileLoad> {
    let staging = directory.join("staging");
    let destination = directory.join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let load = Arc::new(FileLoad::default());
    load.set_window(pool.max_parallel_requests());
    let limits = run_limits();
    let cancellation = CancellationToken::new();
    let runs = files.iter().map(|file| {
        let open = load.start(file.segments.len());
        let (staging, destination, limits, cancellation) =
            (&staging, &destination, &limits, &cancellation);
        async move {
            download_file_counted(
                database,
                pool,
                cancellation,
                file,
                staging,
                destination,
                limits,
                &open,
            )
            .await
        }
    });
    for outcome in futures_util::future::join_all(runs).await {
        assert!(matches!(
            outcome.expect("download"),
            FileOutcome::Completed { missing: 0, .. }
        ));
    }
    assert_eq!(load.running(), 0, "every file left the books when it ended");
    load
}

/// The acceptance case of RD-130-22: a short file ends while a long one still has articles,
/// and no connection sits idle for as much as a round trip while any article is still to be
/// asked for. Told by order at the fixture, not by the clock (RD-1120-09): no connection sat
/// out a whole round trip of another one - a command that arrived after it fell idle and was
/// answered before it was asked again - up to the last command the fixture received, the
/// moment nothing was left to ask for.
#[tokio::test(flavor = "multi_thread")]
async fn no_connection_waits_at_a_file_boundary_while_articles_are_open() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("boundary.sqlite"))
        .await
        .expect("database");
    let (articles, files) = release(&database, &[3, 16]).await;
    // A wall-clock bound on idle failed on loaded runners at 50 ms and at 250 ms (118 ms of
    // idle measured on Windows); the order holds however slow the runner is. The long round
    // trip stays: a connection would have to be starved for all of it to look idle.
    let rtt = Duration::from_millis(250);
    let (address, log) = spawn_fixture(
        articles,
        FixtureTiming {
            rtt,
            bytes_per_second: None,
        },
    )
    .await;
    let pool = NntpPool::new(vec![server(address, 4)]).expect("pool");

    run_together(&database, &pool, &files, directory.path()).await;

    let sat_out = log.round_trips_sat_out();
    assert_eq!(sat_out.len(), 4, "every connection was used");
    for (connection, trips) in sat_out.iter().enumerate() {
        assert_eq!(
            *trips, 0,
            "connection {connection} sat out {trips} round trips of the others while articles were still open"
        );
    }
    assert_eq!(log.requests().len(), 19, "every article was asked for once");
}

/// However many files run, they share the server's connections: never more than its limit,
/// never more commands on one line than the pipeline depth.
#[tokio::test(flavor = "multi_thread")]
async fn files_running_together_never_exceed_the_connection_limit() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("limit.sqlite"))
        .await
        .expect("database");
    let (articles, files) = release(&database, &[4, 4, 4, 4, 4, 4]).await;
    let (address, log) = spawn_fixture(
        articles,
        FixtureTiming {
            rtt: Duration::from_millis(10),
            bytes_per_second: None,
        },
    )
    .await;
    let pool = NntpPool::new(vec![server(address, 2)]).expect("pool");

    let load = run_together(&database, &pool, &files, directory.path()).await;

    assert!(
        log.connection_count() <= 2,
        "{} connections for a limit of two",
        log.connection_count()
    );
    assert!(log.most_outstanding() <= PIPELINE_DEPTH);
    let (_, batches, confirmed) = load.writer_totals();
    assert_eq!(confirmed, 24, "every article was confirmed");
    assert!(batches >= 6, "at least one batch per file");
}

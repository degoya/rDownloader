//! A stop ends the wait for bandwidth (RD-1270-25).
//!
//! The writer waited for an article's whole quota before it looked at the stop again, which it
//! did only between articles. Under a tight limit that wait is as long as the article takes at
//! that rate — and the queue gives a forced removal five seconds.

use std::{sync::Arc, time::Duration};

use rd_core::NzbSegmentState;
use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    test_support::{
        FixtureBehaviour, FixtureTiming, article_set, import_single_file, spawn_fixture_with,
        states,
    },
    worker::{FileOutcome, download_file},
};

#[tokio::test]
async fn a_stop_ends_the_wait_for_bandwidth_at_once() {
    let set = article_set(4, &[]);
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("bandwidth.sqlite"))
        .await
        .expect("database");
    let file = import_single_file(&database, "file.bin", &set.segments).await;
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let (address, log) = spawn_fixture_with(
        Arc::clone(&set.articles),
        FixtureTiming::default(),
        FixtureBehaviour::default(),
    )
    .await;
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
    // 100 B/s: the first article of 3000 bytes waits half a minute for its quota.
    let registry = rd_limits::LimiterRegistry::new();
    registry.set_manual_limit(Some(100));
    let limits = rd_scheduler::RunLimits {
        max_parallel_requests: 32,
        bandwidth: registry.scoped(rd_limits::TransferScope::default()),
        address_policy: None,
    };
    let stop = CancellationToken::new();

    let download = tokio::spawn({
        let database = database.clone();
        let file = file.clone();
        let stop = stop.clone();
        async move {
            download_file(
                &database,
                &pool,
                &stop,
                &file,
                &staging,
                &destination,
                &limits,
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while log.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the first article was asked for");
    // The answer is local and arrives in milliseconds; the writer is then in the wait.
    tokio::time::sleep(Duration::from_millis(300)).await;

    stop.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(2), download)
        .await
        .expect("the stop ended the wait instead of the quota")
        .expect("download task");
    assert!(
        matches!(outcome.expect("download"), FileOutcome::Cancelled),
        "a stopped file ends cancelled"
    );
    assert!(
        states(&database, &file)
            .await
            .iter()
            .all(|state| *state != NzbSegmentState::Completed),
        "the article that waited was not written, so it is fetched again next time"
    );
}

//! Crash and restart for the traffic per server (RD-1100-05) - Axis A of the RD-140-04 recovery
//! matrix, for `usenet.before_traffic_flushed`.
//!
//! The counts live in memory between two flushes. A stop in that window loses them, and must
//! lose nothing else: every flush that committed before stays, and counting after the restart
//! adds to it.

#![cfg(feature = "failpoints")]

use std::sync::atomic::Ordering;

use chrono::Utc;
use rd_core::failpoint::FailpointGuard;
use rd_db::{Database, NewUsenetServer};

use crate::UsenetTraffic;

#[tokio::test]
async fn counts_not_yet_flushed_are_lost_and_nothing_before_them() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("traffic-crash.sqlite"))
        .await
        .expect("database");
    let server = database
        .create_usenet_server(NewUsenetServer {
            name: "Primary".to_owned(),
            host: "news.example.test".to_owned(),
            port: 563,
            tls: true,
            username: None,
            password_ref: None,
            proxy_profile_id: None,
            priority: 0,
            max_connections: 4,
            enabled: true,
        })
        .await
        .expect("server")
        .id;
    let total = |database: Database| async move {
        database
            .list_usenet_server_traffic(Utc::now().date_naive())
            .await
            .expect("traffic")[0]
            .total
    };

    let before = UsenetTraffic::default();
    before.counter(server).fetch_add(1_000, Ordering::AcqRel);
    before.flush(&database).await.expect("first flush");
    before.counter(server).fetch_add(500, Ordering::AcqRel);
    let guard = FailpointGuard::after("usenet.before_traffic_flushed", 0);
    let error = before
        .flush(&database)
        .await
        .expect_err("the crash point stops the flush");
    assert!(guard.fired(), "the crash point was never reached: {error}");
    drop(guard);
    // The process ends here; its counters go with it.
    drop(before);
    assert_eq!(
        total(database.clone()).await,
        1_000,
        "the committed flush stays"
    );

    let after = UsenetTraffic::default();
    assert!(
        after.pending().is_empty(),
        "nothing is carried over from memory"
    );
    after.counter(server).fetch_add(200, Ordering::AcqRel);
    after
        .flush(&database)
        .await
        .expect("flush after the restart");
    assert_eq!(
        total(database).await,
        1_200,
        "counting after the restart adds to what was written, nothing is counted twice"
    );
}

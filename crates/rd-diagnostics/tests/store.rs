//! Capture to store, and the retention that keeps the store bounded (RD-110-02).

use std::sync::Arc;

use rd_core::LogRetentionSettings;
use rd_db::{Database, LogQuery};
use rd_diagnostics::{
    capture::{CaptureStats, LogCaptureLayer},
    sink,
};
use tracing::subscriber::with_default;
use tracing_subscriber::{layer::SubscriberExt, registry};

async fn database(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("diag.sqlite"))
        .await
        .expect("database")
}

#[tokio::test]
async fn what_the_layer_captured_is_what_the_store_holds_and_nothing_more() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let (layer, mut stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    with_default(registry().with(layer), || {
        tracing::info!(download_id = "dl-1", "started");
        tracing::error!(
            code = "http.status",
            url = "https://h.example/f?token=sk-live-4242",
            "failed"
        );
    });
    let stored = sink::drain_now(&mut stream, &database)
        .await
        .expect("drain");
    assert_eq!(stored, 2);

    let records = database
        .query_log_records(&LogQuery {
            limit: 10,
            ..LogQuery::default()
        })
        .await
        .expect("query");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].message, "failed");
    assert_eq!(records[0].code.as_deref(), Some("http.status"));
    assert!(records[0].fields["url"].contains("token=%5Bredacted%5D"));
    assert!(
        !serde_json::to_string(&records)
            .expect("json")
            .contains("sk-live-4242")
    );
    assert_eq!(records[1].correlation_id.as_deref(), Some("dl-1"));
    assert_eq!(
        sink::drain_now(&mut stream, &database)
            .await
            .expect("drain"),
        0,
        "an empty channel writes nothing"
    );
}

#[tokio::test]
async fn the_sweep_applies_the_configured_retention_in_bounded_steps() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let (layer, mut stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    with_default(registry().with(layer), || {
        for index in 0..3_000 {
            tracing::info!(index, "line");
        }
    });
    sink::drain_now(&mut stream, &database)
        .await
        .expect("drain");
    assert_eq!(database.count_log_records().await.expect("count"), 3_000);

    let removed = sink::prune(
        &database,
        &LogRetentionSettings {
            log_retention_records: 1_000,
            log_retention_days: 14,
        },
    )
    .await
    .expect("prune");
    assert_eq!(removed, 2_000);
    assert_eq!(database.count_log_records().await.expect("count"), 1_000);
    let newest = database
        .query_log_records(&LogQuery {
            limit: 1,
            ..LogQuery::default()
        })
        .await
        .expect("query");
    assert_eq!(
        newest[0].fields["index"], "2999",
        "the newest record survives"
    );
}

#[tokio::test]
async fn the_defaults_keep_twenty_thousand_records_for_two_weeks() {
    let defaults = LogRetentionSettings::default();
    assert_eq!(defaults.log_retention_records, 20_000);
    assert_eq!(defaults.log_retention_days, 14);
    assert!(rd_core::LOG_RETENTION_RECORDS_RANGE.contains(&defaults.log_retention_records));
    assert!(rd_core::LOG_RETENTION_DAYS_RANGE.contains(&defaults.log_retention_days));
}

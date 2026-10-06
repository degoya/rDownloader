//! The structured log store (RD-110-02): records survive a restart, the read filters what
//! the viewer asks for, and the prune stays bounded so the queue never waits behind it.

use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use rd_core::LogLevel;
use rd_db::{Database, LogQuery, NewLogRecord};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("logs.sqlite"))
        .await
        .expect("database")
}

fn record(level: LogLevel, component: &str, message: &str) -> NewLogRecord {
    NewLogRecord {
        recorded_at: Utc::now(),
        level,
        component: component.to_owned(),
        code: None,
        correlation_id: None,
        message: message.to_owned(),
        fields: BTreeMap::new(),
    }
}

fn query() -> LogQuery {
    LogQuery {
        limit: 100,
        ..LogQuery::default()
    }
}

#[tokio::test]
async fn records_survive_a_restart_and_come_back_newest_first() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(&directory).await;
        let mut first = record(LogLevel::Info, "rd_http::engine", "first");
        first.code = Some("http.started".to_owned());
        first.correlation_id = Some("dl-1".to_owned());
        first.fields.insert("attempt".to_owned(), "1".to_owned());
        let stored = database
            .append_log_records(vec![first, record(LogLevel::Warn, "rd_usenet", "second")])
            .await
            .expect("append");
        assert_eq!(stored, 2);
    }
    let database = database(&directory).await;
    let records = database.query_log_records(&query()).await.expect("query");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].message, "second");
    assert_eq!(records[0].level, LogLevel::Warn);
    assert_eq!(records[1].message, "first");
    assert_eq!(records[1].code.as_deref(), Some("http.started"));
    assert_eq!(records[1].correlation_id.as_deref(), Some("dl-1"));
    assert_eq!(
        records[1].fields.get("attempt").map(String::as_str),
        Some("1")
    );
    assert_eq!(database.count_log_records().await.expect("count"), 2);
}

#[tokio::test]
async fn the_read_filters_by_level_component_code_correlation_text_and_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut tagged = record(
        LogLevel::Error,
        "rd_http::engine",
        "transfer failed at 100%",
    );
    tagged.code = Some("http.status".to_owned());
    tagged.correlation_id = Some("dl-7".to_owned());
    let mut old = record(LogLevel::Info, "rd_scheduler", "old news");
    old.recorded_at = Utc::now() - Duration::hours(2);
    database
        .append_log_records(vec![
            old,
            record(LogLevel::Debug, "rd_http::chunk", "chunk 3 done"),
            tagged,
            record(
                LogLevel::Info,
                "rd_usenet::worker",
                "article 100_000 fetched",
            ),
        ])
        .await
        .expect("append");

    let warnings = database
        .query_log_records(&LogQuery {
            min_level: Some(LogLevel::Warn),
            ..query()
        })
        .await
        .expect("level");
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].message, "transfer failed at 100%");

    let http = database
        .query_log_records(&LogQuery {
            component: Some("rd_http".to_owned()),
            ..query()
        })
        .await
        .expect("component");
    assert_eq!(http.len(), 2, "a component filter is a prefix match");

    let coded = database
        .query_log_records(&LogQuery {
            code: Some("http.status".to_owned()),
            ..query()
        })
        .await
        .expect("code");
    assert_eq!(coded.len(), 1);

    let correlated = database
        .query_log_records(&LogQuery {
            correlation_id: Some("dl-7".to_owned()),
            ..query()
        })
        .await
        .expect("correlation");
    assert_eq!(correlated.len(), 1);

    // `%` and `_` are characters to a person, not wildcards.
    let percent = database
        .query_log_records(&LogQuery {
            search: Some("100%".to_owned()),
            ..query()
        })
        .await
        .expect("search");
    assert_eq!(percent.len(), 1);
    let underscore = database
        .query_log_records(&LogQuery {
            search: Some("100_000".to_owned()),
            ..query()
        })
        .await
        .expect("search");
    assert_eq!(underscore.len(), 1);
    assert_eq!(underscore[0].component, "rd_usenet::worker");

    let recent = database
        .query_log_records(&LogQuery {
            since: Some(Utc::now() - Duration::minutes(5)),
            ..query()
        })
        .await
        .expect("since");
    assert_eq!(
        recent.len(),
        3,
        "the two-hour-old record is outside the window"
    );

    let all = database.query_log_records(&query()).await.expect("all");
    let page = database
        .query_log_records(&LogQuery {
            before_id: Some(all[0].id),
            limit: 2,
            ..LogQuery::default()
        })
        .await
        .expect("page");
    assert_eq!(page.len(), 2);
    assert!(page.iter().all(|record| record.id < all[0].id));
}

#[tokio::test]
async fn the_prune_keeps_the_newest_records_and_deletes_in_bounded_batches() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    for chunk in 0..25 {
        let batch = (0..200)
            .map(|index| {
                record(
                    LogLevel::Info,
                    "rd_http",
                    &format!("line {}", chunk * 200 + index),
                )
            })
            .collect();
        database.append_log_records(batch).await.expect("append");
    }
    assert_eq!(database.count_log_records().await.expect("count"), 5_000);

    let first = database
        .prune_log_records(1_000, None, 2_000)
        .await
        .expect("prune");
    assert_eq!(first.deleted, 2_000, "one call removes at most one batch");
    assert_eq!(first.remaining_over_cap, 2_000);
    assert_eq!(database.count_log_records().await.expect("count"), 3_000);

    let mut report = first;
    while report.remaining_over_cap > 0 {
        report = database
            .prune_log_records(1_000, None, 2_000)
            .await
            .expect("prune");
    }
    assert_eq!(database.count_log_records().await.expect("count"), 1_000);
    let kept = database.query_log_records(&query()).await.expect("query");
    assert_eq!(kept[0].message, "line 4999", "the newest record is kept");
    let oldest = database
        .query_log_records(&LogQuery {
            limit: 1,
            before_id: Some(kept[0].id - 998),
            ..LogQuery::default()
        })
        .await
        .expect("oldest");
    assert_eq!(
        oldest[0].message, "line 4000",
        "the oldest kept record is the 1000th newest"
    );

    let idle = database
        .prune_log_records(1_000, None, 2_000)
        .await
        .expect("prune");
    assert_eq!(
        idle,
        rd_db::LogPruneReport::default(),
        "nothing to do is reported as nothing"
    );
}

#[tokio::test]
async fn the_prune_removes_records_older_than_the_cutoff_whatever_the_count() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut stale = record(LogLevel::Info, "rd_http", "stale");
    stale.recorded_at = Utc::now() - Duration::days(30);
    database
        .append_log_records(vec![stale, record(LogLevel::Info, "rd_http", "fresh")])
        .await
        .expect("append");

    let report = database
        .prune_log_records(1_000, Some(Utc::now() - Duration::days(14)), 2_000)
        .await
        .expect("prune");
    assert_eq!(report.deleted, 1);
    let kept = database.query_log_records(&query()).await.expect("query");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].message, "fresh");
}

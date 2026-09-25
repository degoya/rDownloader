//! Emptying one store leaves the other two standing (RD-120-34), and emptying the
//! notification history keeps every delivery the worker still owes an attempt (RD-130-08).
//!
//! The store half of the job. What is checked here is the part a REST test cannot reach
//! cheaply: the transfer statistics have no public write on `Database`, so they are seeded
//! through a second connection to the same file, and the three-way isolation is measured as
//! counts rather than inferred.
//!
//! The confirmation, the stable error code and the audit self-entry over REST are in
//! `crates/rd-api/tests/data_reset.rs`.

use std::collections::BTreeMap;

use chrono::Utc;
use rd_core::{AuditAction, AuditActorKind, AuditOutcome, LogLevel};
use rd_db::{
    AuditQuery, CLEARED_DETAIL_KEY, Database, LogQuery, NewAuditRecord, NewDelivery, NewLogRecord,
    StatsResolution,
};
use rd_notify::{DeliveryState, NotificationEvent};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> (Database, std::path::PathBuf) {
    let path = directory.path().join("data-reset.sqlite");
    let database = Database::open(&path).await.expect("database");
    (database, path)
}

fn log_record(message: &str) -> NewLogRecord {
    NewLogRecord {
        recorded_at: Utc::now(),
        level: LogLevel::Info,
        component: "rd_api".to_owned(),
        code: None,
        correlation_id: None,
        message: message.to_owned(),
        fields: BTreeMap::new(),
    }
}

fn audit_record(action: AuditAction) -> NewAuditRecord {
    NewAuditRecord::new(action, AuditOutcome::Success)
}

/// Writes `count` hourly buckets and one all-time total through a second connection.
///
/// `Database` deliberately offers no write for these — they are written inside the
/// transaction that finishes a download — so a test that wants them seeds the file directly.
async fn seed_stats(path: &std::path::Path, count: u32) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("second connection");
    for hour in 0..count {
        let start = StatsResolution::Hour
            .bucket_start(Utc::now() - chrono::Duration::hours(i64::from(hour)));
        sqlx::query(
            "INSERT INTO transfer_stats \
               (resolution, bucket_start, kind, provider, completed, failed, retries, bytes, \
                seconds) \
             VALUES ('hour', ?, 'http', 'direct', 1, 0, 0, 100, 1)",
        )
        .bind(&start)
        .execute(&pool)
        .await
        .expect("seed bucket");
    }
    sqlx::query(
        "INSERT INTO transfer_totals \
           (kind, provider, completed, failed, retries, bytes, seconds) \
         VALUES ('http', 'direct', ?, 0, 0, 100, 1)",
    )
    .bind(i64::from(count))
    .execute(&pool)
    .await
    .expect("seed total");
    pool.close().await;
}

/// Seeds all three stores and returns their counts, so a test can measure a change.
async fn seed_all(database: &Database, path: &std::path::Path) {
    database
        .append_log_records((0..7).map(|i| log_record(&format!("line {i}"))).collect())
        .await
        .expect("logs");
    for action in [
        AuditAction::LoginSucceeded,
        AuditAction::SettingsChanged,
        AuditAction::TokenCreated,
    ] {
        database
            .append_audit_record(audit_record(action))
            .await
            .expect("audit");
    }
    seed_stats(path, 5).await;
}

/// Queues one delivery per entry and moves it to that entry's state; the title is the state's
/// name, so a test can read back which ones survived.
async fn seed_deliveries(database: &Database, states: &[DeliveryState]) {
    let rule_id = rd_core::NotificationRuleId::new();
    let target_id = rd_core::NotificationTargetId::new();
    for (index, state) in states.iter().enumerate() {
        let title = format!("{index}-{state:?}");
        let queued = database
            .queue_notification_delivery(NewDelivery {
                rule_id,
                target_id,
                idempotency_key: format!("seed-{index}"),
                event: NotificationEvent::BudgetExhausted,
                title: title.clone(),
                body: String::new(),
            })
            .await
            .expect("queue");
        assert!(queued, "a fresh idempotency key queues");
        if *state == DeliveryState::Queued {
            continue;
        }
        let delivery = database
            .list_notification_deliveries(50)
            .await
            .expect("list")
            .into_iter()
            .find(|delivery| delivery.title == title)
            .expect("the delivery just queued");
        database
            .record_notification_attempt(
                delivery.id,
                *state,
                1,
                (*state == DeliveryState::Retrying)
                    .then(|| Utc::now() + chrono::Duration::hours(1)),
                None,
                None,
            )
            .await
            .expect("attempt");
    }
}

async fn delivery_titles(database: &Database) -> Vec<String> {
    let mut titles: Vec<String> = database
        .list_notification_deliveries(50)
        .await
        .expect("list")
        .into_iter()
        .map(|delivery| delivery.title)
        .collect();
    titles.sort();
    titles
}

async fn counts(database: &Database) -> (u64, u64, u64) {
    (
        database.count_log_records().await.expect("logs"),
        database.count_audit_records().await.expect("audit"),
        database.count_transfer_stats().await.expect("stats"),
    )
}

#[tokio::test]
async fn clearing_the_logs_leaves_the_audit_and_the_statistics_alone() {
    let directory = TempDir::new().expect("tempdir");
    let (database, path) = database(&directory).await;
    seed_all(&database, &path).await;
    assert_eq!(counts(&database).await, (7, 3, 6));

    let removed = database.clear_log_records().await.expect("clear");

    assert_eq!(removed, 7);
    assert_eq!(counts(&database).await, (0, 3, 6));
    assert!(
        database
            .query_log_records(&LogQuery {
                limit: 50,
                ..LogQuery::default()
            })
            .await
            .expect("query")
            .is_empty()
    );
}

#[tokio::test]
async fn clearing_the_statistics_leaves_the_logs_and_the_audit_alone() {
    let directory = TempDir::new().expect("tempdir");
    let (database, path) = database(&directory).await;
    seed_all(&database, &path).await;

    let removed = database.clear_transfer_stats().await.expect("clear");

    // Five hourly buckets and the one all-time total: both tables, because leaving the total
    // behind would empty every chart and still show a figure nobody could account for.
    assert_eq!(removed, 6);
    assert_eq!(counts(&database).await, (7, 3, 0));
    assert!(
        database
            .list_transfer_totals()
            .await
            .expect("totals")
            .is_empty()
    );
}

#[tokio::test]
async fn clearing_the_audit_leaves_one_record_saying_who_cleared_it() {
    let directory = TempDir::new().expect("tempdir");
    let (database, path) = database(&directory).await;
    seed_all(&database, &path).await;

    let mut marker = audit_record(AuditAction::AuditCleared);
    marker.actor_kind = AuditActorKind::Session;
    marker.actor_id = Some("session-7".to_owned());
    let before = Utc::now();
    let removed = database.clear_audit_records(marker).await.expect("clear");

    assert_eq!(removed, 3, "the three seeded records went");
    // Not zero: the log is empty of what was there and holds exactly the entry that says so.
    let (logs, audit, stats) = counts(&database).await;
    assert_eq!((logs, audit, stats), (7, 1, 6));

    let stored = database
        .query_audit_records(&AuditQuery {
            limit: 50,
            ..AuditQuery::default()
        })
        .await
        .expect("query");
    let entry = stored.first().expect("the clear wrote itself");
    assert_eq!(entry.action, AuditAction::AuditCleared);
    assert_eq!(entry.actor_kind, AuditActorKind::Session);
    assert_eq!(entry.actor_id.as_deref(), Some("session-7"));
    assert_eq!(
        entry.details.get(CLEARED_DETAIL_KEY).map(String::as_str),
        Some("3"),
        "the entry says how many records went: {:?}",
        entry.details
    );
    assert!(
        entry.recorded_at >= before - chrono::Duration::seconds(5),
        "the entry carries the moment of the clear"
    );
}

#[tokio::test]
async fn clearing_the_notification_history_keeps_what_is_still_owed_an_attempt() {
    let directory = TempDir::new().expect("tempdir");
    let (database, path) = database(&directory).await;
    seed_all(&database, &path).await;
    seed_deliveries(
        &database,
        &[
            DeliveryState::Queued,
            DeliveryState::Retrying,
            DeliveryState::Delivered,
            DeliveryState::Failed,
            DeliveryState::Delivered,
        ],
    )
    .await;
    assert_eq!(
        database
            .count_clearable_notification_deliveries()
            .await
            .expect("count"),
        3,
        "the preview counts what a clear would take, not the whole table"
    );

    let removed = database
        .clear_notification_deliveries()
        .await
        .expect("clear");

    assert_eq!(removed, 3, "the two delivered and the one failed went");
    // Deleting either of these would lose the notification itself, not only its record.
    assert_eq!(delivery_titles(&database).await, ["0-Queued", "1-Retrying"]);
    assert_eq!(
        database
            .count_clearable_notification_deliveries()
            .await
            .expect("count"),
        0
    );
    assert_eq!(
        counts(&database).await,
        (7, 3, 6),
        "the logs, the audit and the statistics are other stores"
    );
}

#[tokio::test]
async fn clearing_an_empty_store_is_a_success_that_removed_nothing() {
    let directory = TempDir::new().expect("tempdir");
    let (database, _path) = database(&directory).await;

    assert_eq!(database.clear_log_records().await.expect("logs"), 0);
    assert_eq!(database.clear_transfer_stats().await.expect("stats"), 0);
    assert_eq!(
        database
            .clear_notification_deliveries()
            .await
            .expect("notifications"),
        0
    );
    let removed = database
        .clear_audit_records(audit_record(AuditAction::AuditCleared))
        .await
        .expect("audit");
    assert_eq!(removed, 0);
    // The entry still goes in: an empty audit that nobody emptied and one somebody emptied
    // are different facts, and only the entry can tell them apart.
    assert_eq!(database.count_audit_records().await.expect("count"), 1);
}

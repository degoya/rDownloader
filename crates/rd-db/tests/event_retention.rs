//! The persisted events are purged in bounded batches, so a sweep over a long backlog never
//! holds the writer for one unbounded `DELETE` (DB-09).

use chrono::{Duration, Utc};
use rd_db::Database;
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn old_events_go_in_batches_and_recent_ones_stay() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("events.sqlite");
    let database = Database::open(&path).await.expect("database");
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("connect");
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&mut connection)
        .await
        .expect("busy timeout");
    // More than one batch of events past the retention, and one inside it.
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 4500) \
         INSERT INTO events (id, kind, occurred_at, payload_json) \
         SELECT 'old-' || i, 'download_state', ?, '{}' FROM n",
    )
    .bind(Utc::now() - Duration::days(45))
    .execute(&mut connection)
    .await
    .expect("old events");
    sqlx::query(
        "INSERT INTO events (id, kind, occurred_at, payload_json) \
         VALUES ('recent', 'download_state', ?, '{}')",
    )
    .bind(Utc::now())
    .execute(&mut connection)
    .await
    .expect("recent event");

    assert_eq!(database.purge_old_events().await.expect("purge"), 4500);

    let old: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE id LIKE 'old-%'")
        .fetch_one(&mut connection)
        .await
        .expect("count");
    let recent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE id = 'recent'")
        .fetch_one(&mut connection)
        .await
        .expect("count");
    assert_eq!((old, recent), (0, 1));
    assert_eq!(database.purge_old_events().await.expect("again"), 0);
}

//! Operational notices (RD-190-19): deliveries queued at most once per rule and notice.
//!
//! A bus event is seen once, so its delivery's unique idempotency key is all the protection it
//! needs. A notice comes from a check that repeats -- the update check, the plugin repository
//! refresh, an account check -- and finds the same thing every time. The delivery row cannot
//! remember that: the per-rule trim drops it after [`crate::notify_store::MAX_DELIVERIES_PER_RULE`]
//! newer ones, and the person may clear the history. So the key is also kept here, written in
//! the transaction that queues the delivery: either both exist or neither does, and the second
//! check that finds the same notice queues nothing.

use anyhow::Result;
use chrono::{Duration, Utc};
use sqlx::{Connection, SqliteConnection};

use crate::notify_store::{NewDelivery, queue_delivery};

/// How long a key is remembered. A notice names a version, a dated expiry or a single run, so
/// none of them comes round again after a year; without a bound the table would only grow.
const KEEP_DAYS: i64 = 400;

/// Queues the deliveries whose key this table has not seen, records those keys, and reports
/// how many were queued.
pub(crate) async fn queue_notice(
    connection: &mut SqliteConnection,
    deliveries: Vec<NewDelivery>,
) -> Result<u64> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    sqlx::query("DELETE FROM notification_notices WHERE created_at < ?")
        .bind(now - Duration::days(KEEP_DAYS))
        .execute(&mut *tx)
        .await?;
    let mut queued = 0;
    for delivery in deliveries {
        let event = serde_json::to_string(&delivery.event)?
            .trim_matches('"')
            .to_owned();
        let fresh = sqlx::query(
            "INSERT INTO notification_notices (idempotency_key, rule_id, event, created_at) \
             VALUES (?, ?, ?, ?) ON CONFLICT(idempotency_key) DO NOTHING",
        )
        .bind(&delivery.idempotency_key)
        .bind(delivery.rule_id.to_string())
        .bind(event)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        if fresh && queue_delivery(&mut tx, delivery).await? {
            queued += 1;
        }
    }
    tx.commit().await?;
    Ok(queued)
}

#[cfg(test)]
#[path = "notice_store_tests.rs"]
mod tests;

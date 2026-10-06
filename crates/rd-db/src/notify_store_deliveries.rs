//! The delivery queue's writes: queueing with the per-rule trim, clearing the history,
//! discarding what is still owed, and recording an attempt.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::NotificationDeliveryId;
use rd_notify::DeliveryState;
use sqlx::SqliteConnection;

use super::{MAX_DELIVERIES_PER_RULE, NewDelivery, to_name};

/// Queues a delivery. A key that already exists is left alone, which is what makes one
/// event produce at most one delivery per rule.
pub(crate) async fn queue_delivery(
    connection: &mut SqliteConnection,
    input: NewDelivery,
) -> Result<bool> {
    let now = Utc::now();
    let inserted = sqlx::query(
        "INSERT INTO notification_deliveries (id, rule_id, target_id, idempotency_key, event, \
         title, body, state, attempt, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', 0, ?, ?) \
         ON CONFLICT(idempotency_key) DO NOTHING",
    )
    .bind(NotificationDeliveryId::new().to_string())
    .bind(input.rule_id.to_string())
    .bind(input.target_id.to_string())
    .bind(&input.idempotency_key)
    .bind(to_name(&input.event)?)
    .bind(&input.title)
    .bind(&input.body)
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if inserted.rows_affected() > 0 {
        // Trim in the same write, so the table cannot grow between two inserts. Rows the
        // worker still owes an attempt are held back: dropping one of those would discard the
        // notification itself rather than only its record.
        sqlx::query(
            "DELETE FROM notification_deliveries WHERE rule_id = ? \
             AND state NOT IN ('queued', 'retrying') AND id NOT IN ( \
               SELECT id FROM notification_deliveries WHERE rule_id = ? \
               ORDER BY created_at DESC, rowid DESC LIMIT ? )",
        )
        .bind(input.rule_id.to_string())
        .bind(input.rule_id.to_string())
        .bind(MAX_DELIVERIES_PER_RULE)
        .execute(&mut *connection)
        .await?;
    }
    Ok(inserted.rows_affected() > 0)
}

/// Empties the delivery history on request and reports how many rows went (RD-130-08).
///
/// Rows in `queued` or `retrying` stay, by the rule the per-rule trim in [`queue_delivery`]
/// follows: the worker still owes them an attempt, and deleting one would discard the
/// notification itself rather than only its record.
pub(crate) async fn clear_deliveries(connection: &mut SqliteConnection) -> Result<u64> {
    let deleted = sqlx::query(
        "DELETE FROM notification_deliveries WHERE state NOT IN ('queued', 'retrying')",
    )
    .execute(&mut *connection)
    .await
    .context("clear notification deliveries")?;
    Ok(deleted.rows_affected())
}

/// Cancels the notifications still owed an attempt: deletes every row in `queued` or
/// `retrying` and reports how many went (RD-170-11). The counterpart of [`clear_deliveries`],
/// asked for on purpose.
///
/// An attempt the worker picked up before the delete still runs to its end; its
/// [`record_attempt`] then updates no row and returns `Ok`, so the discarded delivery does not
/// come back.
pub(crate) async fn discard_pending_deliveries(connection: &mut SqliteConnection) -> Result<u64> {
    let deleted =
        sqlx::query("DELETE FROM notification_deliveries WHERE state IN ('queued', 'retrying')")
            .execute(&mut *connection)
            .await
            .context("discard pending notification deliveries")?;
    Ok(deleted.rows_affected())
}

/// Records the outcome of one attempt. A delivery discarded meanwhile has no row left; the
/// update then touches nothing and is still `Ok` (RD-170-11).
pub(crate) async fn record_attempt(
    connection: &mut SqliteConnection,
    id: NotificationDeliveryId,
    state: DeliveryState,
    attempt: u32,
    next_attempt_at: Option<DateTime<Utc>>,
    response_status: Option<u16>,
    response_excerpt: Option<String>,
) -> Result<()> {
    sqlx::query(
        "UPDATE notification_deliveries SET state = ?, attempt = ?, next_attempt_at = ?, \
         response_status = ?, response_excerpt = ?, updated_at = ? WHERE id = ?",
    )
    .bind(to_name(&state)?)
    .bind(i64::from(attempt))
    .bind(next_attempt_at)
    .bind(response_status.map(i64::from))
    .bind(response_excerpt)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *connection)
    .await?;
    Ok(())
}

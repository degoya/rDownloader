//! The archive and poll-run writes: recording items, review states, history and finished runs.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    EventEnvelope, EventKind, SubscriptionHistoryClearResponse, SubscriptionId, SubscriptionItem,
    SubscriptionItemId, SubscriptionItemState, SubscriptionRunId,
};
use sqlx::{Connection, SqliteConnection};

use super::{NewSubscriptionItem, PollResult, changed_event, item_state_string, reason_string};
use crate::{error::StoreError, writer::insert_event};

/// Serializes the attribute map, or `None` when there is nothing to say.
///
/// `None` rather than `"{}"` matters: the upsert coalesces onto the stored value, so an
/// empty object would blank the details of a row a later poll answered without them.
fn attributes_json(attributes: &BTreeMap<String, String>) -> Option<String> {
    if attributes.is_empty() {
        return None;
    }
    serde_json::to_string(attributes).ok()
}

/// The same event, carrying what a finished poll has to say (RD-106-09).
///
/// `changed_event` is anonymous on purpose: every write in this module emits it and a client
/// simply re-reads what it is showing. A finished poll is the one case where that is not
/// enough. The "check now" action answers before the poll runs, so the only thing that can
/// tell the interface *which* subscription stopped being busy — and whether the check found
/// anything — is the event itself. Without these fields a client could not tell a completed
/// poll from an unrelated edit of another subscription.
///
/// Deliberately not named `accepted_items`: that key is what `rd-api`'s automation context
/// looks for, and reviving a trigger is not this event's business.
fn run_finished_event(subscription_id: SubscriptionId, result: &PollResult) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::SubscriptionChanged,
        serde_json::json!({
            "resource": "subscription",
            "poll": "finished",
            "subscription_id": subscription_id.to_string(),
            "found": result.found,
            "accepted": result.accepted,
            "skipped": result.skipped,
            // Already redacted by the caller; it is what the row shows as well.
            "error": result.error,
        }),
    )
}

/// The archive password each recorded row is to get, by row id.
pub(crate) type ItemPasswords = Vec<(String, String)>;

/// Archives the items of one poll and returns the ones that were genuinely new, with the
/// archive password each recorded row is to get.
///
/// `INSERT … ON CONFLICT DO NOTHING` against the UNIQUE index is what makes this safe to
/// repeat: an interrupted poll, an overlapping one, or a feed that re-lists the same entry
/// produces no second row and no second download. The returned list is exactly the rows
/// this call created, so the caller queues only those.
pub(crate) async fn record_items(
    connection: &mut SqliteConnection,
    subscription_id: SubscriptionId,
    items: Vec<NewSubscriptionItem>,
) -> Result<((Vec<SubscriptionItem>, ItemPasswords), EventEnvelope)> {
    let now = Utc::now();
    let mut created = Vec::new();
    let mut passwords = ItemPasswords::new();
    let event = changed_event();
    let mut tx = connection.begin().await?;
    for item in items {
        let id = SubscriptionItemId::new();
        let result = sqlx::query(
            "INSERT INTO subscription_items (id, subscription_id, item_key, title, url, \
             published_at, duration_seconds, state, reason, source_category, media_type, \
             attributes_json, discovered_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(subscription_id, item_key) DO UPDATE SET \
               url = excluded.url, \
               media_type = COALESCE(excluded.media_type, subscription_items.media_type), \
               attributes_json = \
                 COALESCE(excluded.attributes_json, subscription_items.attributes_json) \
             WHERE subscription_items.state = 'pending' \
             RETURNING id",
        )
        .bind(id.to_string())
        .bind(subscription_id.to_string())
        .bind(&item.item_key)
        .bind(&item.title)
        .bind(item.url.as_str())
        .bind(item.published_at)
        .bind(item.duration_seconds.map(i64::from))
        .bind(item_state_string(item.state))
        .bind(item.reason.map(reason_string))
        .bind(item.source_category.as_deref())
        .bind(item.media_type.as_deref())
        // `None` rather than an empty object, so the COALESCE above cannot blank a refreshed
        // row's attributes when a later poll answers without the extended block.
        .bind(attributes_json(&item.attributes))
        .bind(now)
        // The returned id tells an insert from a refresh: an update keeps the row's own id,
        // so only a match on the one just generated is a genuinely new item. `rows_affected`
        // cannot make that distinction and would report every refreshed item as discovered.
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| sqlx::Row::get::<String, _>(&row, "id"));
        // A new row and a refreshed pending one both take the announced password; the vault
        // holds it, `Database::record_subscription_items` puts it there (RD-190-04).
        if let (Some(row), Some(password)) = (&result, &item.password) {
            passwords.push((row.clone(), password.clone()));
        }
        if result.as_deref() == Some(id.to_string().as_str()) {
            created.push(SubscriptionItem {
                id,
                subscription_id,
                item_key: item.item_key,
                title: item.title,
                url: item.url,
                published_at: item.published_at,
                duration_seconds: item.duration_seconds,
                state: item.state,
                reason: item.reason,
                source_category: item.source_category,
                media_type: item.media_type,
                attributes: item.attributes,
                password: item.password,
                discovered_at: now,
            });
        }
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(((created, passwords), event))
}

pub(crate) async fn set_item_state(
    connection: &mut SqliteConnection,
    id: SubscriptionItemId,
    state: SubscriptionItemState,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let result = sqlx::query("UPDATE subscription_items SET state = ? WHERE id = ?")
        .bind(item_state_string(state))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("subscription item not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Dismisses a snapshot of pending items in one transaction.
pub(crate) async fn set_pending_items_state(
    connection: &mut SqliteConnection,
    ids: &[SubscriptionItemId],
    state: SubscriptionItemState,
) -> Result<(u64, EventEnvelope)> {
    if ids.is_empty() {
        return Ok((0, changed_event()));
    }
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let mut updated = 0;
    for id in ids {
        updated += sqlx::query(
            "UPDATE subscription_items SET state = ? WHERE id = ? AND state = 'pending'",
        )
        .bind(item_state_string(state))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((updated, event))
}

/// Deletes settled items and every poll run of one subscription, leaving pending review intact.
pub(crate) async fn clear_history(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
) -> Result<(SubscriptionHistoryClearResponse, EventEnvelope)> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM subscriptions WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?;
    if exists == 0 {
        bail!(StoreError::not_found("subscription not found"));
    }
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let deleted_items = sqlx::query(
        "DELETE FROM subscription_items WHERE subscription_id = ? AND state != 'pending'",
    )
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let deleted_runs = sqlx::query("DELETE FROM subscription_runs WHERE subscription_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((
        SubscriptionHistoryClearResponse {
            deleted_items,
            deleted_runs,
        },
        event,
    ))
}

/// Writes the poll's outcome and the next due time in one transaction.
///
/// `primed` is set here and never cleared: after one completed poll the backlog decision has
/// been made, and re-applying it later would silently discard everything published since.
/// A failed poll does not prime (RD-190-13): it decided nothing, and a first poll that a rate
/// limit or a network error stopped must leave the backlog decision to the next one, or that
/// one takes the whole history as new.
pub(crate) async fn finish_run(
    connection: &mut SqliteConnection,
    subscription_id: SubscriptionId,
    started_at: DateTime<Utc>,
    result: PollResult,
) -> Result<EventEnvelope> {
    let now = Utc::now();
    let event = run_finished_event(subscription_id, &result);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO subscription_runs (id, subscription_id, started_at, finished_at, found, \
         accepted, skipped, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(SubscriptionRunId::new().to_string())
    .bind(subscription_id.to_string())
    .bind(started_at)
    .bind(now)
    .bind(i64::from(result.found))
    .bind(i64::from(result.accepted))
    .bind(i64::from(result.skipped))
    .bind(result.error.as_deref())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE subscriptions SET last_run_at = ?, next_run_at = ?, consecutive_failures = ?, \
         last_error = ?, etag = COALESCE(?, etag), last_modified = COALESCE(?, last_modified), \
         primed = CASE WHEN ? IS NULL THEN 1 ELSE primed END, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(result.next_run_at)
    .bind(i64::from(result.consecutive_failures))
    .bind(result.error.as_deref())
    .bind(result.etag.as_deref())
    .bind(result.last_modified.as_deref())
    .bind(result.error.as_deref())
    .bind(now)
    .bind(subscription_id.to_string())
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

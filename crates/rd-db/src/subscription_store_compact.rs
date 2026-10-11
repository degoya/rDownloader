//! Compacting the archive (RD-1240-35): a skipped or dismissed item older than the retention
//! leaves only its key behind, in `subscription_item_keys`.
//!
//! The archive is the once-only guarantee — a poll recognises an item by `(subscription_id,
//! item_key)` — and on a busy indexer subscription it was also the second-largest table: 169 k
//! rows in a month, 135 k of them skipped by the filters and 34 k dismissed, each with its title,
//! address and details. The key is what the guarantee needs; the rest only served the history
//! page. So the key moves and the row goes, in one transaction: there is no moment in which the
//! item is in neither table. Pending and queued items are never touched.

use std::ops::RangeInclusive;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rd_core::EventEnvelope;
use serde::Deserialize;
use sqlx::{Connection, SqliteConnection, SqlitePool};

use super::changed_event;
use crate::writer::insert_event;

/// Days a skipped or dismissed item keeps its full row by default.
pub const DEFAULT_ITEM_RETENTION_DAYS: u32 = 30;
/// Bounds of `subscription_item_retention_days`; 0 keeps every row whole.
pub const ITEM_RETENTION_DAYS_RANGE: RangeInclusive<u32> = 0..=3650;

/// The archive's slice of the settings document.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct SubscriptionItemRetention {
    /// Days a skipped or dismissed item keeps its full row before only its key stays.
    pub subscription_item_retention_days: u32,
}

impl Default for SubscriptionItemRetention {
    fn default() -> Self {
        Self {
            subscription_item_retention_days: DEFAULT_ITEM_RETENTION_DAYS,
        }
    }
}

impl SubscriptionItemRetention {
    /// The days in force: the default for a stored value out of range, so a broken value never
    /// stops the clean-up.
    #[must_use]
    pub fn days(&self) -> u32 {
        if ITEM_RETENTION_DAYS_RANGE.contains(&self.subscription_item_retention_days) {
            self.subscription_item_retention_days
        } else {
            DEFAULT_ITEM_RETENTION_DAYS
        }
    }

    /// Items discovered before this are compacted; `None` when the retention keeps them all.
    #[must_use]
    pub fn compact_before(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self.days() {
            0 => None,
            days => Some(now - Duration::days(i64::from(days))),
        }
    }
}

/// Items one compaction batch moves at most, so a first pass over a long archive never holds the
/// writer for long — the same bound the event purge uses.
pub(crate) const COMPACTION_BATCH: i64 = 2_000;

/// Moves at most [`COMPACTION_BATCH`] settled items discovered before `before` to their keys and
/// returns how many went, with the change to announce when any did — the counts and the history
/// page a client shows change. The caller repeats while a batch comes back full.
pub(crate) async fn compact_items(
    connection: &mut SqliteConnection,
    before: DateTime<Utc>,
) -> Result<(u64, Option<EventEnvelope>)> {
    let mut tx = connection.begin().await?;
    // The batch is read once and named by rowid, so the copy and the delete act on exactly the
    // same rows.
    let rowids: Vec<i64> = sqlx::query_scalar(
        "SELECT rowid FROM subscription_items \
         WHERE state IN ('skipped', 'dismissed') AND discovered_at < ? LIMIT ?",
    )
    .bind(before)
    .bind(COMPACTION_BATCH)
    .fetch_all(&mut *tx)
    .await?;
    if rowids.is_empty() {
        return Ok((0, None));
    }
    let batch = serde_json::to_string(&rowids)?;
    sqlx::query(
        "INSERT OR IGNORE INTO subscription_item_keys (subscription_id, item_key) \
         SELECT subscription_id, item_key FROM subscription_items \
         WHERE rowid IN (SELECT value FROM json_each(?))",
    )
    .bind(&batch)
    .execute(&mut *tx)
    .await?;
    // A row with an archive password hands it to the sweep through the delete trigger.
    let removed = sqlx::query(
        "DELETE FROM subscription_items WHERE rowid IN (SELECT value FROM json_each(?))",
    )
    .bind(&batch)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let event = changed_event();
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((removed, Some(event)))
}

/// Settled items discovered before `before`: what a compaction would move now.
pub(crate) async fn compactable_items(pool: &SqlitePool, before: DateTime<Utc>) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM subscription_items \
         WHERE state IN ('skipped', 'dismissed') AND discovered_at < ?",
    )
    .bind(before)
    .fetch_one(pool)
    .await?;
    Ok(u64::try_from(count).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{DEFAULT_ITEM_RETENTION_DAYS, SubscriptionItemRetention};

    #[test]
    fn the_retention_reads_its_default_keeps_on_zero_and_falls_back_out_of_range() {
        let now = Utc::now();
        let parsed: SubscriptionItemRetention =
            serde_json::from_value(serde_json::json!({ "other": 1 })).expect("slice");
        assert_eq!(parsed.days(), DEFAULT_ITEM_RETENTION_DAYS);
        assert_eq!(parsed.compact_before(now), Some(now - Duration::days(30)));
        let keep = SubscriptionItemRetention {
            subscription_item_retention_days: 0,
        };
        assert_eq!(keep.compact_before(now), None);
        let broken = SubscriptionItemRetention {
            subscription_item_retention_days: 99_999,
        };
        assert_eq!(broken.days(), DEFAULT_ITEM_RETENTION_DAYS);
    }
}

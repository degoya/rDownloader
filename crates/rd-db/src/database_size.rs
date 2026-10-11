//! What the database file holds and how it gets smaller (RD-1240-35).
//!
//! Deleting rows does not shrink an SQLite file: the pages go to its free list and wait for the
//! next row. Two ways give them back. `PRAGMA incremental_vacuum` moves pages from the end of the
//! file into free ones and cuts the end off — cheap, but only in a file whose `auto_vacuum` is
//! `INCREMENTAL`, which every file this build creates is ([`Database::open`]). A file created
//! before needs one `VACUUM` to take that on: a rewrite of the whole file, which needs room for
//! a second copy and holds the writer until it is done. [`Database::reclaim_free_pages`] is the
//! first, [`Database::rewrite`] the second; the clean-up in `rd-api-admin` decides when.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::task::yield_now;

use crate::{
    Database,
    commands::{MaintenanceCommand, SubscriptionsCommand},
    subscription_store::{COMPACTION_BATCH, compactable_items},
    writer,
};

/// The file and the two stores that grow with use.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatabaseStorage {
    /// The database's size in pages times the page size: the file once the WAL is checkpointed.
    pub file_bytes: u64,
    /// Pages on the free list: what the file would lose if they were handed back.
    pub free_bytes: u64,
    /// Whether the file is `auto_vacuum = INCREMENTAL`, so the free pages go back without a
    /// rewrite.
    pub incremental: bool,
    /// The persisted events, with their indexes.
    pub event_rows: u64,
    pub event_bytes: u64,
    /// The subscription archive's full rows, with their indexes.
    pub item_rows: u64,
    pub item_bytes: u64,
    /// The keys compacted items left behind.
    pub item_key_rows: u64,
    pub item_key_bytes: u64,
    /// Skipped or dismissed items a compaction with the cut-off asked for would move now, and
    /// their share of `item_bytes` (an estimate: the rows are averaged).
    pub compactable_items: u64,
    pub compactable_bytes: u64,
}

impl Database {
    /// Measures the file and its growing stores; `compact_before` is the cut-off a compaction
    /// would use, `None` when the retention keeps every item.
    pub async fn database_storage(
        &self,
        compact_before: Option<DateTime<Utc>>,
    ) -> Result<DatabaseStorage> {
        let pool = &self.readers;
        let page_size = scalar(pool, "PRAGMA page_size").await?;
        let item_rows = scalar(pool, "SELECT COUNT(*) FROM subscription_items").await?;
        let item_bytes = table_bytes(pool, "subscription_items").await?;
        let compactable = match compact_before {
            Some(before) => compactable_items(pool, before).await?,
            None => 0,
        };
        Ok(DatabaseStorage {
            file_bytes: scalar(pool, "PRAGMA page_count").await? * page_size,
            free_bytes: scalar(pool, "PRAGMA freelist_count").await? * page_size,
            incremental: scalar(pool, "PRAGMA auto_vacuum").await? == 2,
            event_rows: scalar(pool, "SELECT COUNT(*) FROM events").await?,
            event_bytes: table_bytes(pool, "events").await?,
            item_rows,
            item_bytes,
            item_key_rows: scalar(pool, "SELECT COUNT(*) FROM subscription_item_keys").await?,
            item_key_bytes: table_bytes(pool, "subscription_item_keys").await?,
            compactable_items: compactable,
            compactable_bytes: share(item_bytes, compactable, item_rows),
        })
    }

    /// Moves every skipped or dismissed subscription item discovered before `before` to its key
    /// (RD-1240-35), in bounded batches with a yield between them so the writer serves the queue
    /// in the gaps. Returns how many items went; their archive passwords leave the vault.
    pub async fn compact_subscription_items(&self, before: DateTime<Utc>) -> Result<u64> {
        let mut removed = 0;
        loop {
            let batch = writer::request(&self.writer, |reply| {
                SubscriptionsCommand::CompactSubscriptionItems { before, reply }
            })
            .await?;
            removed += batch;
            if batch < COMPACTION_BATCH.unsigned_abs() {
                break;
            }
            yield_now().await;
        }
        if removed > 0 {
            self.sweep_archive_passwords().await;
        }
        Ok(removed)
    }

    /// Hands the free pages back to the file system, when the file is
    /// `auto_vacuum = INCREMENTAL`; returns the bytes the file shrank by.
    pub async fn reclaim_free_pages(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| MaintenanceCommand::ReclaimFreePages {
            reply,
        })
        .await
    }

    /// Rewrites the whole file without its free pages; the file takes on
    /// `auto_vacuum = INCREMENTAL` with it. Holds the writer until it is done and needs room for
    /// a second copy of the file — the caller checks both.
    pub async fn rewrite(&self) -> Result<()> {
        writer::request(&self.writer, |reply| MaintenanceCommand::Vacuum { reply }).await?;
        self.checkpoint_wal().await
    }
}

/// The one number a pragma or a count answers.
async fn scalar(pool: &SqlitePool, statement: &'static str) -> Result<u64> {
    let value: i64 = sqlx::query_scalar(statement).fetch_one(pool).await?;
    Ok(u64::try_from(value).unwrap_or_default())
}

/// The pages of a table and its indexes, from the `dbstat` table the bundled SQLite carries.
async fn table_bytes(pool: &SqlitePool, table: &'static str) -> Result<u64> {
    let bytes: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(pgsize) FROM dbstat WHERE aggregate = 1 \
         AND name IN (SELECT name FROM sqlite_schema WHERE tbl_name = ?)",
    )
    .bind(table)
    .fetch_one(pool)
    .await?;
    Ok(u64::try_from(bytes.unwrap_or_default()).unwrap_or_default())
}

/// `part` of `count` rows' share of `bytes`.
fn share(bytes: u64, part: u64, count: u64) -> u64 {
    if count == 0 {
        return 0;
    }
    u64::try_from(u128::from(bytes) * u128::from(part.min(count)) / u128::from(count))
        .unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::share;

    #[test]
    fn a_share_is_proportional_and_never_more_than_the_whole() {
        assert_eq!(share(1_000, 250, 1_000), 250);
        assert_eq!(share(1_000, 5, 0), 0);
        assert_eq!(share(1_000, 2_000, 1_000), 1_000);
        assert_eq!(share(u64::MAX, 1, 2), u64::MAX / 2);
    }
}

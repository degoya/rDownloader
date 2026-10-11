//! Writer housekeeping: crash recovery at start, WAL checkpoint, snapshots and settings.

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::Connection;

use super::remove_package_if_empty;
use crate::writer::Writer;

impl Writer {
    pub(crate) async fn recover_interrupted(&mut self) -> Result<u64> {
        let now = Utc::now();
        let mut transaction = self.connection.begin().await?;
        // A row held back for the PAR2 verdict (RD-108-24) is `verifying` and must stay
        // where it is: its file is whole on disk apart from the holes, and requeueing it
        // would fetch the whole file again to arrive at the same open question.
        let result = sqlx::query(
            "UPDATE downloads SET state = 'queued', updated_at = ? \
             WHERE state IN ('resolving', 'downloading', 'repairing') \
             OR (state = 'verifying' \
                 AND (last_error_json IS NULL OR last_error_json NOT LIKE ?))",
        )
        .bind(now)
        .bind(crate::nzb_queue::AWAITING_PAR2_PATTERN)
        .execute(&mut *transaction)
        .await?;
        // Extraction runs on finished files; an interrupted extraction must not re-download.
        sqlx::query(
            "UPDATE downloads SET state = 'completed', updated_at = ? WHERE state = 'extracting'",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE nzb_segments SET state = 'queued' WHERE state = 'downloading'")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE postprocess_steps SET state = 'queued' WHERE state = 'running'")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE link_candidates SET state = 'online' WHERE state IN ('resolving', 'checking')",
        )
        .execute(&mut *transaction)
        .await?;
        // A package row is always written before its first file — `rd_scheduler::enqueue` and
        // every other creator do it in that order — so a process that stops in that window
        // leaves a package with nothing in it. Nothing in the queue or the interface tells
        // such a row apart from a package that is simply short, so it reads as a finished
        // package that downloaded nothing, and it can never be removed the ordinary way
        // because removal hangs off a file it does not have. `delete_download` already treats
        // a package whose last file is gone as gone; this applies the same rule at the start.
        let empty: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM packages WHERE NOT EXISTS \
             (SELECT 1 FROM downloads WHERE package_id = packages.id)",
        )
        .fetch_all(&mut *transaction)
        .await?;
        for package_id in &empty {
            remove_package_if_empty(&mut transaction, package_id).await?;
        }
        transaction.commit().await?;
        // Whatever the set was waiting for before the restart, it is not running now. A
        // package whose other rows all reached a terminal state before the process stopped
        // gets its verdict here; one whose rows were just requeued keeps waiting for them.
        let waiting =
            crate::nzb_queue::packages_awaiting_par2_verdict(&mut self.connection).await?;
        for package_id in waiting {
            self.settle_package_after_download(package_id).await?;
        }
        Ok(result.rows_affected())
    }

    pub(crate) async fn checkpoint_wal(&mut self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut self.connection)
            .await?;
        Ok(())
    }

    /// `PRAGMA incremental_vacuum`, then the checkpoint that carries the shorter file over from
    /// the WAL (RD-1240-35). Moves the pages at the end of the file into the free ones, so it
    /// costs about as much as the free pages it returns; in a file without
    /// `auto_vacuum = INCREMENTAL` it does nothing. Returns the bytes the file shrank by.
    pub(crate) async fn reclaim_free_pages(&mut self) -> Result<u64> {
        let pages_before = self.page_count().await?;
        sqlx::query("PRAGMA incremental_vacuum")
            .execute(&mut self.connection)
            .await?;
        self.checkpoint_wal().await?;
        let pages_after = self.page_count().await?;
        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&mut self.connection)
            .await?;
        Ok(u64::try_from((pages_before - pages_after).max(0) * page_size).unwrap_or_default())
    }

    async fn page_count(&mut self) -> Result<i64> {
        Ok(sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&mut self.connection)
            .await?)
    }

    /// `VACUUM INTO` on the writer's own connection: the copy is the state after every
    /// command sent before this one, and no command sent after it (see `crate::snapshot`).
    pub(crate) async fn vacuum_into(&mut self, path: &std::path::Path) -> Result<()> {
        // SQLite refuses a target that already holds data; refusing earlier names the reason.
        anyhow::ensure!(
            !path.exists(),
            "the snapshot target {} already exists",
            path.display()
        );
        let target = path
            .to_str()
            .with_context(|| format!("snapshot path {} is not UTF-8", path.display()))?;
        sqlx::query("VACUUM INTO ?")
            .bind(target)
            .execute(&mut self.connection)
            .await
            .with_context(|| format!("write database snapshot {}", path.display()))?;
        Ok(())
    }

    /// Writes `value` under `key` unless the key already holds one; `false` when it did.
    ///
    /// One statement, so two callers racing for the same key cannot both see it empty and both
    /// write -- the check and the write are the same row lock.
    pub(crate) async fn insert_setting_if_absent(
        &mut self,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<bool> {
        let written = sqlx::query(
            "INSERT INTO settings (key, value_json, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO NOTHING",
        )
        .bind(key)
        .bind(serde_json::to_string(value)?)
        .bind(Utc::now())
        .execute(&mut self.connection)
        .await?
        .rows_affected();
        Ok(written == 1)
    }

    pub(crate) async fn set_setting(&mut self, key: &str, value: &serde_json::Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value_json, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(serde_json::to_string(value)?)
        .bind(Utc::now())
        .execute(&mut self.connection)
        .await?;
        Ok(())
    }
}

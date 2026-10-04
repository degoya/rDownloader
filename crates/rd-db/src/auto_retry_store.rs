//! Limit waits and the automatic retry of failed downloads (RD-191-12): the two counters a
//! download keeps for them beside `retry_count`, the failed rows the retry reads, and its two
//! writes.
//!
//! Neither counter is part of [`DownloadFile`]: only the scheduler reads them, and only at the
//! two moments that decide on them — a failure being recorded and the retry's pass.

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind};
use sqlx::{Connection, Row, SqlitePool};

use crate::{
    Database,
    commands::WriterCommand,
    writer::{self, Writer, insert_event},
};

/// The counters of one download that `DownloadFile` does not carry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetryCounters {
    /// Consecutive waits for a limit the hoster imposed; they do not spend `retry_count`.
    pub limit_waits: u32,
    /// How often the automatic retry has put the download back into the queue.
    pub auto_retry_rounds: u32,
}

/// A failed download as the automatic retry sees it.
#[derive(Clone, Debug)]
pub struct AutoRetryCandidate {
    pub download: DownloadFile,
    /// Rounds the automatic retry has already spent on it.
    pub rounds: u32,
}

async fn retry_counters(pool: &SqlitePool, id: DownloadId) -> Result<Option<RetryCounters>> {
    let row = sqlx::query("SELECT limit_waits, auto_retry_rounds FROM downloads WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    row.map(|row| {
        Ok(RetryCounters {
            limit_waits: u32::try_from(row.get::<i64, _>("limit_waits"))
                .context("invalid limit wait count")?,
            auto_retry_rounds: u32::try_from(row.get::<i64, _>("auto_retry_rounds"))
                .context("invalid automatic retry count")?,
        })
    })
    .transpose()
}

async fn failed_rounds(pool: &SqlitePool) -> Result<HashMap<String, u32>> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT id, auto_retry_rounds FROM downloads WHERE state = 'failed'")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, rounds)| (id, u32::try_from(rounds).unwrap_or(u32::MAX)))
        .collect())
}

impl Database {
    /// The limit-wait and automatic-retry counters of one download, `None` if it is gone.
    pub async fn retry_counters(&self, id: DownloadId) -> Result<Option<RetryCounters>> {
        retry_counters(&self.readers, id).await
    }

    /// Every failed download with the rounds the automatic retry spent on it, in queue order.
    pub async fn auto_retry_candidates(&self) -> Result<Vec<AutoRetryCandidate>> {
        let downloads = crate::models::failed_downloads(&self.readers).await?;
        let rounds = failed_rounds(&self.readers).await?;
        Ok(downloads
            .into_iter()
            .map(|download| AutoRetryCandidate {
                rounds: rounds
                    .get(&download.id.to_string())
                    .copied()
                    .unwrap_or_default(),
                download,
            })
            .collect())
    }

    /// Sets or clears when the automatic retry takes a failed download up again; the time is
    /// kept in `next_retry_at`, which is what the queue shows. Only a row still `failed` is
    /// written; returns whether one was.
    pub async fn schedule_auto_retry(
        &self,
        id: DownloadId,
        at: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::ScheduleAutoRetry {
            id,
            at,
            reply,
        })
        .await
    }

    /// Puts a failed download back into the queue as a round of the automatic retry: the
    /// attempts and limit waits start from zero, the round is counted. `None` when the row is
    /// gone or no longer `failed` — a person resumed or removed it meanwhile.
    pub async fn auto_retry_download(&self, id: DownloadId) -> Result<Option<DownloadFile>> {
        writer::request(&self.writer, |reply| WriterCommand::RequeueFailed {
            id,
            auto_retry: true,
            reply,
        })
        .await
    }

    /// Puts a failed download back into the queue by a person's hand: the same fresh budget as
    /// a round of the automatic retry, without counting a round. `None` when the row is gone
    /// or no longer `failed`.
    pub async fn retry_failed_download(&self, id: DownloadId) -> Result<Option<DownloadFile>> {
        writer::request(&self.writer, |reply| WriterCommand::RequeueFailed {
            id,
            auto_retry: false,
            reply,
        })
        .await
    }
}

impl Writer {
    pub(crate) async fn schedule_auto_retry(
        &mut self,
        id: DownloadId,
        at: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        // No `state` in the payload: the row's state does not change, and a `failed` here
        // would fire the "download failed" automations a second time.
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "next_retry_at": at, "auto_retry": true }),
        );
        let mut transaction = self.connection.begin().await?;
        let written = sqlx::query(
            "UPDATE downloads SET next_retry_at = ? WHERE id = ? AND state = 'failed' \
             AND next_retry_at IS NOT ?",
        )
        .bind(at)
        .bind(id.to_string())
        .bind(at)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if written == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        Ok(true)
    }

    /// A failed download back to `queued` with a fresh retry budget; `auto_retry` counts the
    /// round, a person's resume does not.
    pub(crate) async fn requeue_failed(
        &mut self,
        id: DownloadId,
        auto_retry: bool,
    ) -> Result<Option<DownloadFile>> {
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({
                "download_id": id,
                "previous": DownloadState::Failed,
                "state": DownloadState::Queued,
                "auto_retry": auto_retry,
            }),
        );
        let mut transaction = self.connection.begin().await?;
        // The guard on the state is the whole concurrency story: the caller read the row a
        // moment ago, and a resume or removal in between leaves nothing to do here.
        let written = sqlx::query(
            "UPDATE downloads SET state = 'queued', retry_count = 0, limit_waits = 0, \
             next_retry_at = NULL, last_error_json = NULL, block_reason = NULL, \
             auto_retry_rounds = auto_retry_rounds + ?, updated_at = ? \
             WHERE id = ? AND state = 'failed'",
        )
        .bind(i64::from(auto_retry))
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if written == 0 {
            transaction.rollback().await?;
            return Ok(None);
        }
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        let updated = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context("download disappeared after it was queued again")?;
        // The package may have settled as failed with this file; it is waiting again now.
        self.settle_package_after_download(updated.package_id)
            .await?;
        Ok(Some(updated))
    }
}

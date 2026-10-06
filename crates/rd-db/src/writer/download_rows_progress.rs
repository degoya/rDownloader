//! Progress and chunk-plan writes of the writer: chunk checkpoints, runner progress, the
//! throttled `download.progress` event, chunk MACs and the chunk plan itself.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{ChunkId, DownloadId, EventEnvelope, EventKind};
use sqlx::{Connection, Row};

use crate::error::StoreError;
use crate::models::PersistedChunk;
use crate::writer::{PROGRESS_EVENT_INTERVAL, Writer};

impl Writer {
    pub(in crate::writer) async fn checkpoint_chunk(
        &mut self,
        chunk_id: ChunkId,
        committed_offset: u64,
    ) -> Result<()> {
        let value = i64::try_from(committed_offset).context("chunk offset exceeds SQLite range")?;
        let mut tx = self.connection.begin().await?;
        let row = sqlx::query(
            "SELECT download_id, start_offset, end_offset, committed_offset FROM chunks WHERE id = ?",
        )
        .bind(chunk_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .context("chunk not found")?;
        let previous: i64 = row.get("committed_offset");
        let start: i64 = row.get("start_offset");
        let end: Option<i64> = row.get("end_offset");
        if value < previous || value < start || end.is_some_and(|limit| value > limit) {
            bail!("invalid chunk checkpoint");
        }
        sqlx::query("UPDATE chunks SET committed_offset = ?, updated_at = ? WHERE id = ?")
            .bind(value)
            .bind(Utc::now())
            .bind(chunk_id.to_string())
            .execute(&mut *tx)
            .await?;
        let download_id: String = row.get("download_id");
        sqlx::query(
            "UPDATE downloads SET committed_bytes = (SELECT COALESCE(SUM(committed_offset - start_offset), 0) \
             FROM chunks WHERE download_id = ?), updated_at = ? WHERE id = ?",
        )
        .bind(&download_id)
        .bind(Utc::now())
        .bind(&download_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.broadcast_progress(&download_id);
        Ok(())
    }

    /// Progress written by runners that do not use chunk rows (media downloads).
    pub(in crate::writer) async fn set_download_progress(
        &mut self,
        id: DownloadId,
        committed_bytes: u64,
        total_bytes: Option<u64>,
    ) -> Result<()> {
        let committed = i64::try_from(committed_bytes).context("progress exceeds SQLite range")?;
        let total = total_bytes
            .map(i64::try_from)
            .transpose()
            .context("total exceeds SQLite range")?;
        sqlx::query(
            "UPDATE downloads SET committed_bytes = ?, total_bytes = COALESCE(?, total_bytes), \
             updated_at = ? WHERE id = ?",
        )
        .bind(committed)
        .bind(total)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut self.connection)
        .await?;
        self.broadcast_progress(&id.to_string());
        Ok(())
    }

    /// Emits at most one `download.progress` event per download and interval.
    pub(in crate::writer) fn broadcast_progress(&mut self, download_id: &str) {
        let now = std::time::Instant::now();
        let due = self
            .last_progress
            .get(download_id)
            .is_none_or(|last| now.duration_since(*last) >= PROGRESS_EVENT_INTERVAL);
        if !due {
            return;
        }
        self.last_progress.insert(download_id.to_owned(), now);
        if self.last_progress.len() > 1024 {
            self.last_progress
                .retain(|_, last| now.duration_since(*last) < PROGRESS_EVENT_INTERVAL * 10);
        }
        let _ = self.events.send(EventEnvelope::new(
            EventKind::DownloadProgress,
            serde_json::json!({ "download_id": download_id }),
        ));
    }

    /// Writes one finished provider-chunk MAC (RD-103-02, ADR 0011).
    ///
    /// The fingerprint is stored with it and every row of another description is removed in
    /// the same transaction: the condensed value needs all the chunk MACs of *one* stream,
    /// so a half-and-half set would verify nothing and refuse a correct file.
    pub(in crate::writer) async fn checkpoint_chunk_mac(
        &mut self,
        download_id: DownloadId,
        fingerprint: &str,
        index: u64,
        mac: [u8; 16],
    ) -> Result<()> {
        let index = i64::try_from(index).context("chunk index exceeds SQLite range")?;
        let mut tx = self.connection.begin().await?;
        sqlx::query("DELETE FROM transform_chunk_macs WHERE download_id = ? AND fingerprint <> ?")
            .bind(download_id.to_string())
            .bind(fingerprint)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO transform_chunk_macs (download_id, chunk_index, mac, fingerprint, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(download_id, chunk_index) DO UPDATE SET \
             mac = excluded.mac, fingerprint = excluded.fingerprint, updated_at = excluded.updated_at",
        )
        .bind(download_id.to_string())
        .bind(index)
        .bind(mac.to_vec())
        .bind(fingerprint)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(in crate::writer) async fn prepare_transfer(
        &mut self,
        id: DownloadId,
        total_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        chunks: Vec<PersistedChunk>,
    ) -> Result<()> {
        let total = total_bytes
            .map(|value| i64::try_from(value).context("file size exceeds SQLite range"))
            .transpose()?;
        let mut tx = self.connection.begin().await?;
        let committed =
            sqlx::query_scalar::<_, i64>("SELECT committed_bytes FROM downloads WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .context(StoreError::not_found("download not found"))?;
        if committed != 0 {
            bail!("cannot replace chunk plan after committed progress");
        }
        sqlx::query("DELETE FROM chunks WHERE download_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        // A new chunk plan is a new run of this file from zero. Chunk MACs accumulated for
        // the old one describe bytes nobody is going to write again.
        sqlx::query("DELETE FROM transform_chunk_macs WHERE download_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        for chunk in chunks {
            sqlx::query(
                "INSERT INTO chunks (id, download_id, start_offset, end_offset, committed_offset, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(chunk.id.to_string())
            .bind(id.to_string())
            .bind(i64::try_from(chunk.start)?)
            .bind(chunk.end.map(i64::try_from).transpose()?)
            .bind(i64::try_from(chunk.committed)?)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE downloads SET total_bytes = ?, etag = ?, last_modified = ?, updated_at = ? WHERE id = ?",
        )
        .bind(total)
        .bind(etag)
        .bind(last_modified)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

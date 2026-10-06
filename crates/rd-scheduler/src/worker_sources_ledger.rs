//! The ledger a multi-source run writes what it learns to: the download's own rows.

use anyhow::Result;
use async_trait::async_trait;
use rd_core::SourceOutcome;
use rd_db::Database;
use rd_http::{CheckpointSink, SourceLedger};

/// Writes what the engine learns straight to the download's rows.
pub(super) struct DatabaseLedger {
    pub(super) database: Database,
    pub(super) download_id: rd_core::DownloadId,
}

#[async_trait]
impl CheckpointSink for DatabaseLedger {
    async fn commit(&self, chunk_id: rd_core::ChunkId, committed_offset: u64) -> Result<()> {
        self.database
            .checkpoint_chunk(chunk_id, committed_offset)
            .await
    }
}

#[async_trait]
impl SourceLedger for DatabaseLedger {
    async fn source_delivered(&self, position: u32, bytes: u64) -> Result<()> {
        self.database
            .record_source_outcome(
                self.download_id,
                position,
                SourceOutcome::Delivered { bytes },
            )
            .await
    }

    async fn source_failed(
        &self,
        position: u32,
        code: &str,
        retry_after_seconds: Option<u64>,
    ) -> Result<()> {
        self.database
            .record_source_outcome(
                self.download_id,
                position,
                SourceOutcome::Failed {
                    code: code.to_owned(),
                    retry_after_seconds,
                },
            )
            .await
    }

    async fn source_isolated(&self, position: u32, code: &str) -> Result<()> {
        // A position the row does not know — a mark left by a set that was since replaced —
        // has nothing to isolate; the rewind that follows still happens.
        match self
            .database
            .record_source_outcome(
                self.download_id,
                position,
                SourceOutcome::Isolated {
                    code: code.to_owned(),
                },
            )
            .await
        {
            Err(error) if rd_db::store_kind(&error) == Some(rd_db::StoreErrorKind::NotFound) => {
                Ok(())
            }
            other => other,
        }
    }

    async fn chunk_marked(
        &self,
        chunk_id: rd_core::ChunkId,
        position: Option<u32>,
        verified: bool,
    ) -> Result<()> {
        self.database.mark_chunk(chunk_id, position, verified).await
    }

    async fn chunk_rewound(&self, chunk_id: rd_core::ChunkId, committed: u64) -> Result<()> {
        self.database.rewind_chunk(chunk_id, committed).await
    }
}

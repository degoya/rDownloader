//! Database facade methods for the persistent transfer statistics (RD-110-01).

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::{
    Database,
    commands::WriterCommand,
    stats_store::{
        self, StatsPruneReport, StatsResolution, StatsRetention, TransferBucket, TransferTotal,
    },
    writer,
};

impl Database {
    /// Every bucket at `resolution` from `since` on, oldest first.
    pub async fn list_transfer_stats(
        &self,
        resolution: StatsResolution,
        since: DateTime<Utc>,
    ) -> Result<Vec<TransferBucket>> {
        stats_store::list_buckets(&self.readers, resolution, since).await
    }

    /// The all-time totals by kind and provider; what the metrics counters read.
    pub async fn list_transfer_totals(&self) -> Result<Vec<TransferTotal>> {
        stats_store::list_totals(&self.readers).await
    }

    /// How many rows the statistics hold, buckets and all-time totals together.
    pub async fn count_transfer_stats(&self) -> Result<u64> {
        stats_store::count_rows(&self.readers).await
    }

    /// Empties both statistics tables and reports how many rows went (RD-120-34).
    pub async fn clear_transfer_stats(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::ClearTransferStats {
            reply,
        })
        .await
    }

    /// One bounded pass of the retention sweep; repeat while the report says `more`.
    pub async fn prune_transfer_stats(
        &self,
        retention: StatsRetention,
    ) -> Result<StatsPruneReport> {
        writer::request(&self.writer, |reply| WriterCommand::PruneTransferStats {
            retention,
            reply,
        })
        .await
    }
}

//! Assembly checkpoints written in batches (RD-130-22).

use anyhow::Result;
use rd_db::Database;

use crate::parallel::OpenArticles;

/// Articles confirmed in one writer transaction at most (RD-130-22).
///
/// A batch only fills while articles arrive faster than the writer confirms them: the
/// assembly confirms whatever it holds before it waits for the network, so on a line the
/// writer keeps up with, every article is still confirmed on its own, the moment it is
/// written. Under load the batch turns two transactions per article - the `Downloading` mark
/// before the request and the checkpoint after it - into one per sixteen. The bound is what
/// a crash may cost: the articles of an unconfirmed batch are on disk but fetched again,
/// which is the invariant of `usenet.before_checkpoint_batch`.
const CHECKPOINT_BATCH: usize = 16;

/// Articles written to the `.part` file whose checkpoints wait for the next writer batch.
#[derive(Default)]
pub(crate) struct PendingCheckpoints {
    /// The yEnc name and size of the article pushed last; every article of a file announces
    /// the same size, and the name is the one the old per-article checkpoint left behind.
    name: String,
    declared_size: u64,
    segments: Vec<rd_db::AssembledSegment>,
}

impl PendingCheckpoints {
    pub(crate) fn push(
        &mut self,
        name: String,
        declared_size: u64,
        segment: rd_db::AssembledSegment,
    ) {
        self.name = name;
        self.declared_size = declared_size;
        self.segments.push(segment);
    }

    pub(crate) fn is_full(&self) -> bool {
        self.segments.len() >= CHECKPOINT_BATCH
    }

    /// Confirms every waiting article in one transaction.
    ///
    /// The batch is taken out before it is written: one that fails is gone, and its
    /// articles - on disk, never confirmed - are fetched again by the resume, exactly as
    /// after a crash at the same instant. Nothing retries it into a half-confirmed state.
    pub(crate) async fn flush(
        &mut self,
        database: &Database,
        file_id: rd_core::NzbFileId,
        open: &OpenArticles,
    ) -> Result<()> {
        if self.segments.is_empty() {
            return Ok(());
        }
        let segments = std::mem::take(&mut self.segments);
        let count = segments.len();
        rd_core::failpoint!("usenet.before_checkpoint_batch", || {
            anyhow::anyhow!("crash point: usenet.before_checkpoint_batch")
        });
        let started = std::time::Instant::now();
        database
            .checkpoint_nzb_assembly_segments(
                file_id,
                self.name.clone(),
                self.declared_size,
                segments,
            )
            .await?;
        open.wrote(count, started.elapsed());
        Ok(())
    }

    /// [`Self::flush`] on the way out of a file that stops for another reason; that reason
    /// is what the caller reports, so a batch that fails here is only logged.
    pub(crate) async fn flush_before_leaving(
        &mut self,
        database: &Database,
        file_id: rd_core::NzbFileId,
        open: &OpenArticles,
    ) {
        if let Err(error) = self.flush(database, file_id, open).await {
            tracing::warn!(
                nzb_file_id = %file_id,
                %error,
                "written articles stay unconfirmed and will be fetched again"
            );
        }
    }
}

//! Download sources and the chunk marks built on them (RD-150-03): the writer half of
//! `download_sources_store`.

use chrono::Utc;

use super::{Writer, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_sources(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::RecordSourceOutcome {
                download_id,
                position,
                outcome,
                reply,
            } => {
                send(
                    reply,
                    crate::download_sources_store::record_outcome(
                        &mut self.connection,
                        download_id,
                        position,
                        &outcome,
                        Utc::now(),
                    )
                    .await,
                );
            }
            WriterCommand::MarkChunk {
                chunk_id,
                source_position,
                verified,
                reply,
            } => {
                send(
                    reply,
                    crate::download_sources_store::mark_chunk(
                        &mut self.connection,
                        chunk_id,
                        source_position,
                        verified,
                    )
                    .await,
                );
            }
            WriterCommand::RewindChunk {
                chunk_id,
                committed,
                reply,
            } => {
                let result = self.rewind_chunk(chunk_id, committed).await;
                send(reply, result);
            }
            WriterCommand::SetCandidateSourceSet {
                candidate_id,
                set,
                reply,
            } => {
                send(
                    reply,
                    crate::download_sources_store::set_candidate_source_set(
                        &mut self.connection,
                        candidate_id,
                        &set,
                    )
                    .await,
                );
            }
            WriterCommand::SetCandidateRemoteReach {
                candidate_ids,
                local_network,
                reply,
            } => {
                send(
                    reply,
                    crate::download_sources_store::set_candidates_remote_reach(
                        &mut self.connection,
                        &candidate_ids,
                        local_network,
                    )
                    .await,
                );
            }
            // `Writer::run` routes every variant to exactly one handler; see
            // `handle_plugins` for why a mis-routed command is dropped, not a panic.
            _ => {}
        }
    }

    /// Rewinds a chunk in one transaction with the download's total, then tells the
    /// interface: the progress bar has to go back as visibly as the bytes did.
    async fn rewind_chunk(
        &mut self,
        chunk_id: rd_core::ChunkId,
        committed: u64,
    ) -> anyhow::Result<()> {
        use sqlx::Connection as _;
        let mut tx = self.connection.begin().await?;
        let download_id =
            crate::download_sources_store::rewind_chunk(&mut tx, chunk_id, committed).await?;
        tx.commit().await?;
        self.broadcast_progress(&download_id);
        Ok(())
    }
}

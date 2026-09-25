//! Download rows, chunks and the transfer state machine.
//!
//! Also the two `torrent_store` writes and the three `replay_store` ones: they are grouped by
//! the store that owns the SQL, which is where a reader looks for them.

use super::{Writer, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_downloads(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::CreatePackage { package, reply } => {
                send(reply, self.create_package(package).await);
            }
            WriterCommand::CreateDownload { download, reply } => {
                send(reply, self.create_download(download).await);
            }
            WriterCommand::TransitionDownload { id, next, reply } => {
                send(reply, self.transition_download(id, next).await);
            }
            WriterCommand::BlockDownload { id, reason, reply } => {
                send(reply, self.block_download(id, reason).await);
            }
            WriterCommand::DeleteDownload { id, reply } => {
                send(reply, self.delete_download(id).await);
            }
            WriterCommand::DeleteEmptyPackage { id, reply } => {
                send(reply, self.delete_empty_package(id).await);
            }
            WriterCommand::CheckpointChunk {
                chunk_id,
                committed_offset,
                reply,
            } => {
                send(
                    reply,
                    self.checkpoint_chunk(chunk_id, committed_offset).await,
                );
            }
            WriterCommand::CheckpointChunkMac {
                download_id,
                fingerprint,
                index,
                mac,
                reply,
            } => {
                send(
                    reply,
                    self.checkpoint_chunk_mac(download_id, &fingerprint, index, mac)
                        .await,
                );
            }
            WriterCommand::SetDownloadProgress {
                id,
                committed_bytes,
                total_bytes,
                reply,
            } => {
                send(
                    reply,
                    self.set_download_progress(id, committed_bytes, total_bytes)
                        .await,
                );
            }
            WriterCommand::SetCandidateTorrentState { id, state, reply } => {
                let result =
                    crate::torrent_store::set_candidate_state(&mut self.connection, id, &state)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetDownloadTorrentState { id, state, reply } => {
                let result =
                    crate::torrent_store::set_download_state(&mut self.connection, id, &state)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::PrepareTransfer {
                id,
                total_bytes,
                etag,
                last_modified,
                chunks,
                reply,
            } => {
                send(
                    reply,
                    self.prepare_transfer(id, total_bytes, etag, last_modified, chunks)
                        .await,
                );
            }
            WriterCommand::RecordFailure {
                id,
                failure,
                retry_at,
                reply,
            } => {
                send(reply, self.record_failure(id, failure, retry_at).await);
            }
            WriterCommand::CompleteDownload {
                id,
                final_name,
                checksum,
                reply,
            } => {
                send(
                    reply,
                    self.complete_download(id, &final_name, checksum.as_ref())
                        .await,
                );
            }
            WriterCommand::RenameDownload {
                id,
                file_name,
                reply,
            } => {
                send(reply, self.rename_download(id, &file_name).await);
            }
            WriterCommand::SetFileName {
                id,
                file_name,
                reply,
            } => {
                send(reply, self.set_file_name(id, &file_name).await);
            }
            WriterCommand::SetTransformKeyRef {
                id,
                reference,
                reply,
            } => {
                send(reply, self.set_transform_key_ref(id, reference).await);
            }
            WriterCommand::ClaimResolverRefresh { id, reply } => {
                send(reply, self.claim_resolver_refresh(id).await);
            }
            WriterCommand::ClaimReplayRefresh { id, reply } => {
                let now = chrono::Utc::now();
                send(
                    reply,
                    crate::replay_store::claim_replay_refresh(&mut self.connection, id, now).await,
                );
            }
            WriterCommand::ResetDownload { id, reply } => {
                send(reply, self.reset_download(id).await);
            }
            WriterCommand::ResetTransfer { id, reply } => {
                let now = chrono::Utc::now();
                send(
                    reply,
                    crate::replay_store::reset_transfer(&mut self.connection, id, now).await,
                );
            }
            WriterCommand::SetCandidateReplayConsent { id, consent, reply } => {
                send(
                    reply,
                    crate::replay_store::set_candidate_consent(
                        &mut self.connection,
                        id,
                        consent.as_ref().as_ref(),
                    )
                    .await,
                );
            }
            WriterCommand::ClaimResolverPin { id, pin, reply } => {
                send(reply, self.claim_resolver_pin(id, pin).await);
            }
            WriterCommand::ClearUnsatisfiableResolverPins { available, reply } => {
                send(
                    reply,
                    self.clear_unsatisfiable_resolver_pins(&available).await,
                );
            }
            // `Writer::run` routes every variant to exactly one handler, and its match is
            // exhaustive over `WriterCommand`, so nothing reaches this arm. It drops the
            // command instead of panicking: a mis-routed command must not take down the one
            // task every mutation in the process runs on, and the caller already treats a
            // dropped reply as a failed request.
            _ => {}
        }
    }
}

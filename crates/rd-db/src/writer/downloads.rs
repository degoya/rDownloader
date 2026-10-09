//! Download rows, chunks and the transfer state machine.
//!
//! Also the two `torrent_store` writes and the three `replay_store` ones: they are grouped by
//! the store that owns the SQL, which is where a reader looks for them.

use super::{Writer, publish_unit_event, send};
use crate::commands::DownloadsCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_downloads(&mut self, command: DownloadsCommand) {
        match command {
            DownloadsCommand::CreatePackage { package, reply } => {
                send(reply, self.create_package(package).await);
            }
            DownloadsCommand::CreateDownload {
                download,
                sources,
                reply,
            } => {
                send(reply, self.create_download(download, sources).await);
            }
            DownloadsCommand::AnnounceCreated {
                package_id,
                ids,
                reply,
            } => {
                send(reply, self.announce_created(package_id, &ids).await);
            }
            DownloadsCommand::TransitionDownload { id, next, reply } => {
                send(reply, self.transition_download(id, next).await);
            }
            DownloadsCommand::JoinQueue {
                id,
                created_at,
                reply,
            } => {
                send(reply, self.join_queue(id, created_at).await);
            }
            DownloadsCommand::BlockDownload { id, reason, reply } => {
                send(reply, self.block_download(id, reason).await);
            }
            DownloadsCommand::DeleteDownload { id, reply } => {
                send(reply, self.delete_download(id).await);
            }
            DownloadsCommand::DeleteDownloads { ids, reply } => {
                send(reply, self.delete_downloads(&ids).await);
            }
            DownloadsCommand::DeleteEmptyPackage { id, reply } => {
                send(reply, self.delete_empty_package(id).await);
            }
            DownloadsCommand::CheckpointChunk {
                chunk_id,
                committed_offset,
                reply,
            } => {
                send(
                    reply,
                    self.checkpoint_chunk(chunk_id, committed_offset).await,
                );
            }
            DownloadsCommand::CheckpointChunkMac {
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
            DownloadsCommand::SetDownloadProgress {
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
            DownloadsCommand::SetCandidateTorrentState { id, state, reply } => {
                let result =
                    crate::torrent_store::set_candidate_state(&mut self.connection, id, &state)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            DownloadsCommand::SetDownloadTorrentState { id, state, reply } => {
                let result =
                    crate::torrent_store::set_download_state(&mut self.connection, id, &state)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            DownloadsCommand::PrepareTransfer {
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
            DownloadsCommand::RecordFailure {
                id,
                failure,
                retry_at,
                reply,
            } => {
                send(reply, self.record_failure(id, failure, retry_at).await);
            }
            DownloadsCommand::ScheduleAutoRetry { id, at, reply } => {
                send(reply, self.schedule_auto_retry(id, at).await);
            }
            DownloadsCommand::RequeueFailed {
                id,
                auto_retry,
                reply,
            } => {
                send(reply, self.requeue_failed(id, auto_retry).await);
            }
            DownloadsCommand::CompleteDownload {
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
            DownloadsCommand::RenameDownload {
                id,
                file_name,
                reply,
            } => {
                send(reply, self.rename_download(id, &file_name).await);
            }
            DownloadsCommand::SetFileName {
                id,
                file_name,
                reply,
            } => {
                send(reply, self.set_file_name(id, &file_name).await);
            }
            DownloadsCommand::SetTransformKeyRef {
                id,
                reference,
                reply,
            } => {
                send(reply, self.set_transform_key_ref(id, reference).await);
            }
            DownloadsCommand::ClaimResolverRefresh { id, reply } => {
                send(reply, self.claim_resolver_refresh(id).await);
            }
            DownloadsCommand::ClaimReplayRefresh { id, reply } => {
                let now = chrono::Utc::now();
                send(
                    reply,
                    crate::replay_store::claim_replay_refresh(&mut self.connection, id, now).await,
                );
            }
            DownloadsCommand::ResetDownload { id, reply } => {
                send(reply, self.reset_download(id).await);
            }
            DownloadsCommand::ResetTransfer { id, reply } => {
                let now = chrono::Utc::now();
                send(
                    reply,
                    crate::replay_store::reset_transfer(&mut self.connection, id, now).await,
                );
            }
            DownloadsCommand::SetCandidateReplayConsent { id, consent, reply } => {
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
            DownloadsCommand::ClaimResolverPin { id, pin, reply } => {
                send(reply, self.claim_resolver_pin(id, pin).await);
            }
            DownloadsCommand::ClearUnsatisfiableResolverPins { available, reply } => {
                send(
                    reply,
                    self.clear_unsatisfiable_resolver_pins(&available).await,
                );
            }
            DownloadsCommand::PinDownloadResolver { id, pin, reply } => {
                send(reply, self.pin_download_resolver(id, pin).await);
            }
            DownloadsCommand::ReleaseResolverPin { id, reply } => {
                send(reply, self.release_resolver_pin(id).await);
            }
        }
    }
}

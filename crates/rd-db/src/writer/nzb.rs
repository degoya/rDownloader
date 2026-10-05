//! NZB imports and the Usenet post-processing checkpoints: the writer half of `nzb_store`,
//! `nzb_queue` and `postprocess_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::NzbCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_nzb(&mut self, command: NzbCommand) {
        match command {
            NzbCommand::SettleNzbRecovery {
                id,
                file_name,
                content_is_par2,
                reply,
            } => {
                let result = crate::nzb_queue::settle_recovery(
                    &mut self.connection,
                    id,
                    &file_name,
                    content_is_par2,
                )
                .await
                .map(|events| {
                    let postponed = events.len();
                    for event in events {
                        let _ = self.events.send(event);
                    }
                    postponed
                });
                send(reply, result);
            }
            NzbCommand::DeferPar2Verdict { id, missing, reply } => {
                let result =
                    crate::nzb_queue::defer_par2_verdict(&mut self.connection, id, missing).await;
                send(reply, result);
            }
            NzbCommand::AddNzbImport { import, reply } => {
                let result = crate::nzb_store::add_import(&mut self.connection, import).await;
                if let Ok((_, events)) = &result {
                    for event in events {
                        let _ = self.events.send(event.clone());
                    }
                }
                send(reply, result.map(|(import, _)| import));
            }
            NzbCommand::RecordNzbImportFailure { failure, reply } => {
                let result =
                    crate::nzb_store::record_import_failure(&mut self.connection, failure).await;
                if let Ok((_, Some(event))) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(import, _)| import));
            }
            NzbCommand::UpdateNzbImport { id, change, reply } => {
                let result =
                    crate::nzb_store::update_import(&mut self.connection, id, change).await;
                publish_config(reply, result, &self.events);
            }
            NzbCommand::MarkNzbImportRemoteJob {
                id,
                remote_job_id,
                expected,
                reply,
            } => {
                let result = crate::nzb_store::mark_remote_job(
                    &mut self.connection,
                    id,
                    remote_job_id,
                    expected,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            NzbCommand::DeleteNzbImport { id, reply } => {
                let result = crate::nzb_store::delete_import(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            NzbCommand::ForgetNzbImportHistory { package_id, reply } => {
                let result =
                    crate::nzb_store::forget_import_for_package(&mut self.connection, package_id)
                        .await;
                if let Ok(Some(event)) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|_| ()));
            }
            NzbCommand::SetNzbSegmentState {
                id,
                state,
                crc32,
                reply,
            } => {
                let result =
                    crate::nzb_store::set_segment_state(&mut self.connection, id, state, crc32)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            NzbCommand::EnqueueNzbImport {
                id,
                destination,
                priority,
                start_paused,
                reply,
            } => {
                let result = crate::nzb_queue::enqueue_import(
                    &mut self.connection,
                    id,
                    &destination,
                    priority,
                    start_paused,
                )
                .await
                .map(|(package_id, events)| {
                    for event in events {
                        let _ = self.events.send(event);
                    }
                    package_id
                });
                send(reply, result);
            }
            NzbCommand::CheckpointNzb { checkpoint, reply } => {
                let progress_file = match &checkpoint {
                    crate::postprocess_store::NzbCheckpoint::AssemblySegments {
                        file_id, ..
                    } => Some(file_id.to_string()),
                    _ => None,
                };
                let result =
                    crate::postprocess_store::apply(&mut self.connection, checkpoint).await;
                if result.is_ok()
                    && let Some(file_id) = progress_file
                    && let Ok(Some(download_id)) = sqlx::query_scalar::<_, String>(
                        "SELECT id FROM downloads WHERE nzb_file_id = ?",
                    )
                    .bind(file_id)
                    .fetch_optional(&mut self.connection)
                    .await
                {
                    self.broadcast_progress(&download_id);
                }
                publish_unit_event(reply, result, &self.events);
            }
            NzbCommand::FailHopelessPackage {
                package_id,
                failure,
                reply,
            } => {
                let result = async {
                    let events = crate::nzb_hopeless::fail_hopeless(
                        &mut self.connection,
                        package_id,
                        failure,
                    )
                    .await?;
                    for event in events {
                        let _ = self.events.send(event);
                    }
                    // The rows just failed may have been the last the package waited for.
                    self.settle_package_after_download(package_id).await
                }
                .await;
                send(reply, result);
            }
        }
    }
}

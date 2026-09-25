//! The LinkGrabber: the writer half of `collector_store`, `collector_packages` and `collector_media`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_collector(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::AddMediaCandidates {
                package_id,
                entries,
                reply,
            } => {
                let result = crate::collector_media::add_media_candidates(
                    &mut self.connection,
                    package_id,
                    entries,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetCandidateEnrichment { id, fields, reply } => {
                let result =
                    crate::collector_store::set_enrichment(&mut self.connection, id, &fields).await;
                send(reply, result);
            }
            WriterCommand::SetCandidateMediaInventory { id, state, reply } => {
                let result =
                    crate::collector_media::set_media_inventory(&mut self.connection, id, *state)
                        .await;
                send(reply, result);
            }
            WriterCommand::SetCandidateMediaSelection { id, update, reply } => {
                let result =
                    crate::collector_media::set_media_selection(&mut self.connection, id, *update)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetCandidateMediaVariant {
                id,
                variant_id,
                reply,
            } => {
                let result = crate::collector_media::set_media_variant(
                    &mut self.connection,
                    id,
                    &variant_id,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetCandidateProvider {
                id,
                provider,
                reply,
            } => {
                let result =
                    crate::collector_media::set_provider(&mut self.connection, id, &provider).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetCandidateAuthProfile {
                id,
                selection,
                reply,
            } => {
                let result =
                    crate::collector_media::set_auth_profile(&mut self.connection, id, selection)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::AddCollectorBatch {
                intake,
                secret_fragment_refs,
                reply,
            } => {
                let result = crate::collector_store::add_batch(
                    &mut self.connection,
                    intake,
                    secret_fragment_refs,
                )
                .await
                .map(|(batch, packages, candidates, events)| {
                    for event in events {
                        let _ = self.events.send(event);
                    }
                    (batch, packages, candidates)
                });
                send(reply, result);
            }
            WriterCommand::UpdateCollectorPackages { ids, change, reply } => {
                let result =
                    crate::collector_packages::update(&mut self.connection, &ids, &change).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::ReorderCollectorPackages { ids, reply } => {
                let result = crate::collector_packages::reorder(&mut self.connection, &ids).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ReorderGrabberEntries {
                entries,
                after,
                reply,
            } => {
                let result = crate::collector_packages::reorder_entries(
                    &mut self.connection,
                    &entries,
                    after,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ReorderCandidates {
                package_id,
                ids,
                reply,
            } => {
                let result = crate::collector_packages::reorder_candidates(
                    &mut self.connection,
                    package_id,
                    &ids,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::MoveCandidates { ids, target, reply } => {
                let result =
                    crate::collector_packages::move_candidates(&mut self.connection, &ids, target)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteCollectorPackage { id, reply } => {
                let result = crate::collector_packages::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::RegroupBatches { batch_ids, reply } => {
                let result =
                    crate::collector_packages::regroup(&mut self.connection, &batch_ids).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ClaimCandidatesForCheck { ids, reply } => {
                send(
                    reply,
                    crate::collector_packages::claim_for_check(&mut self.connection, &ids).await,
                );
            }
            WriterCommand::RecordCandidateCheck {
                id,
                result,
                error,
                was_duplicate,
                cached_by,
                reply,
            } => {
                let outcome = crate::collector_packages::record_check(
                    &mut self.connection,
                    id,
                    result,
                    error,
                    was_duplicate,
                    cached_by,
                )
                .await;
                publish_unit_event(reply, outcome, &self.events);
            }
            WriterCommand::SetMirrorPreference { preference, reply } => {
                let result =
                    crate::collector_mirrors::store_preference(&mut self.connection, &preference)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetMirrorPin { id, pinned, reply } => {
                let result =
                    crate::collector_mirrors::store_pin(&mut self.connection, id, pinned).await;
                // Publishes only when something changed: a link that is in no mirror group is
                // a refusal, and an event for it would make every listener re-read the list
                // for nothing.
                if let Ok(Some(event)) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|event| event.is_some()));
            }
            WriterCommand::DissolveMirrorGroup { id, reply } => {
                let result =
                    crate::collector_mirrors::store_dissolve(&mut self.connection, id).await;
                // Only a dissolve that happened is worth an event; a refusal would make every
                // listener re-read the list to find nothing changed.
                if let Ok((_, Some(event))) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(outcome, _)| outcome));
            }
            WriterCommand::MarkCandidateUnsupported {
                id,
                message,
                cached_by,
                reply,
            } => {
                let outcome = crate::collector_packages::mark_unsupported(
                    &mut self.connection,
                    id,
                    message,
                    cached_by,
                )
                .await;
                publish_unit_event(reply, outcome, &self.events);
            }
            WriterCommand::SetCandidateFileName {
                id,
                file_name,
                reply,
            } => {
                let result =
                    crate::collector_packages::set_file_name(&mut self.connection, id, &file_name)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::ClaimPackageForEnqueue { id, only, reply } => {
                send(
                    reply,
                    crate::collector_packages::claim_package_for_enqueue(
                        &mut self.connection,
                        id,
                        only.as_deref(),
                    )
                    .await,
                );
            }
            WriterCommand::FinishPackageEnqueue {
                id,
                success,
                restore,
                reply,
            } => {
                let result = crate::collector_packages::finish_package_enqueue(
                    &mut self.connection,
                    id,
                    success,
                    &restore,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::DeleteCandidate { id, reply } => {
                let result =
                    crate::collector_store::delete_candidate(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::DeleteCandidates { reply } => {
                let result = crate::collector_store::delete_candidates(&mut self.connection).await;
                publish_config(reply, result, &self.events);
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

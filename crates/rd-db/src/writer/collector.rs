//! The LinkGrabber: the writer half of `collector_store`, `collector_packages`, `collector_media`
//! and `link_filter_apply`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::CollectorCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_collector(&mut self, command: CollectorCommand) {
        match command {
            command @ (CollectorCommand::AddMediaCandidates { .. }
            | CollectorCommand::SetCandidateEnrichment { .. }
            | CollectorCommand::SetCandidateMediaInventory { .. }
            | CollectorCommand::SetCandidateMediaSelection { .. }
            | CollectorCommand::SetCandidateMediaVariant { .. }
            | CollectorCommand::SetCandidateProvider { .. }
            | CollectorCommand::SetCandidateAuthProfile { .. }) => {
                self.collector_candidate_fields(command).await
            }
            command @ (CollectorCommand::AddCollectorBatch { .. }
            | CollectorCommand::DeleteCollectorPackage { .. }
            | CollectorCommand::RegroupBatches { .. }
            | CollectorCommand::DeleteCandidate { .. }
            | CollectorCommand::DeleteCandidates { .. }) => self.collector_intake(command).await,
            command @ (CollectorCommand::UpdateCollectorPackages { .. }
            | CollectorCommand::ReorderCollectorPackages { .. }
            | CollectorCommand::ReorderGrabberEntries { .. }
            | CollectorCommand::ReorderCandidates { .. }
            | CollectorCommand::MoveCandidates { .. }
            | CollectorCommand::ApplyLinkFilters { .. }
            | CollectorCommand::ShowFilteredCandidates { .. }) => {
                self.collector_order(command).await
            }
            command @ (CollectorCommand::ClaimCandidatesForCheck { .. }
            | CollectorCommand::RecordCandidateCheck { .. }
            | CollectorCommand::MarkCandidateUnsupported { .. }
            | CollectorCommand::SetCandidateFileName { .. }) => self.collector_check(command).await,
            command @ (CollectorCommand::SetMirrorPreference { .. }
            | CollectorCommand::SetMirrorPin { .. }
            | CollectorCommand::DissolveMirrorGroup { .. }) => {
                self.collector_mirrors(command).await
            }
            command @ (CollectorCommand::ClaimPackageForEnqueue { .. }
            | CollectorCommand::FinishPackageEnqueue { .. }) => {
                self.collector_enqueue(command).await
            }
        }
    }

    /// The fields of one link candidate: its media, enrichment, provider and sign-in profile.
    async fn collector_candidate_fields(&mut self, command: CollectorCommand) {
        match command {
            CollectorCommand::AddMediaCandidates {
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
            CollectorCommand::SetCandidateEnrichment { id, fields, reply } => {
                let result =
                    crate::collector_store::set_enrichment(&mut self.connection, id, &fields).await;
                send(reply, result);
            }
            CollectorCommand::SetCandidateMediaInventory { id, state, reply } => {
                let result =
                    crate::collector_media::set_media_inventory(&mut self.connection, id, *state)
                        .await;
                send(reply, result);
            }
            CollectorCommand::SetCandidateMediaSelection { id, update, reply } => {
                let result =
                    crate::collector_media::set_media_selection(&mut self.connection, id, *update)
                        .await;
                publish_config(reply, result, &self.events);
            }
            CollectorCommand::SetCandidateMediaVariant {
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
            CollectorCommand::SetCandidateProvider {
                id,
                provider,
                reply,
            } => {
                let result =
                    crate::collector_media::set_provider(&mut self.connection, id, &provider).await;
                publish_config(reply, result, &self.events);
            }
            CollectorCommand::SetCandidateAuthProfile {
                id,
                selection,
                reply,
            } => {
                let result =
                    crate::collector_media::set_auth_profile(&mut self.connection, id, selection)
                        .await;
                publish_config(reply, result, &self.events);
            }
            _ => unreachable!("routed by handle_collector"),
        }
    }

    /// Batches coming in, and candidates and packages going away.
    async fn collector_intake(&mut self, command: CollectorCommand) {
        match command {
            CollectorCommand::AddCollectorBatch {
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
                .map(|(batch, packages, candidates, passwords, events)| {
                    for event in events {
                        let _ = self.events.send(event);
                    }
                    (batch, packages, candidates, passwords)
                });
                send(reply, result);
            }
            CollectorCommand::DeleteCollectorPackage { id, reply } => {
                let result = crate::collector_packages::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            CollectorCommand::RegroupBatches { batch_ids, reply } => {
                let result =
                    crate::collector_packages::regroup(&mut self.connection, &batch_ids).await;
                publish_unit_event(reply, result, &self.events);
            }
            CollectorCommand::DeleteCandidate { id, reply } => {
                let result =
                    crate::collector_store::delete_candidate(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            CollectorCommand::DeleteCandidates { reply } => {
                let result = crate::collector_store::delete_candidates(&mut self.connection).await;
                publish_config(reply, result, &self.events);
            }
            _ => unreachable!("routed by handle_collector"),
        }
    }

    /// The packages' settings and the order and grouping of packages and candidates.
    async fn collector_order(&mut self, command: CollectorCommand) {
        match command {
            CollectorCommand::UpdateCollectorPackages { ids, change, reply } => {
                let result =
                    crate::collector_packages::update(&mut self.connection, &ids, &change).await;
                publish_config(reply, result, &self.events);
            }
            CollectorCommand::ReorderCollectorPackages { ids, reply } => {
                let result = crate::collector_packages::reorder(&mut self.connection, &ids).await;
                publish_unit_event(reply, result, &self.events);
            }
            CollectorCommand::ReorderGrabberEntries {
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
            CollectorCommand::ReorderCandidates {
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
            CollectorCommand::MoveCandidates { ids, target, reply } => {
                let result =
                    crate::collector_packages::move_candidates(&mut self.connection, &ids, target)
                        .await;
                publish_config(reply, result, &self.events);
            }
            CollectorCommand::ApplyLinkFilters { reply } => {
                let result =
                    crate::link_filter_apply::apply_link_filters(&mut self.connection).await;
                publish_config(reply, result, &self.events);
            }
            CollectorCommand::ShowFilteredCandidates { ids, reply } => {
                let result =
                    crate::link_filter_apply::show_filtered_candidates(&mut self.connection, &ids)
                        .await;
                publish_config(reply, result, &self.events);
            }
            _ => unreachable!("routed by handle_collector"),
        }
    }

    /// The online check: claiming candidates and recording what it found.
    async fn collector_check(&mut self, command: CollectorCommand) {
        match command {
            CollectorCommand::ClaimCandidatesForCheck { ids, reply } => {
                send(
                    reply,
                    crate::collector_packages::claim_for_check(&mut self.connection, &ids).await,
                );
            }
            CollectorCommand::RecordCandidateCheck {
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
            CollectorCommand::MarkCandidateUnsupported {
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
            CollectorCommand::SetCandidateFileName {
                id,
                file_name,
                reply,
            } => {
                let result =
                    crate::collector_packages::set_file_name(&mut self.connection, id, &file_name)
                        .await;
                publish_config(reply, result, &self.events);
            }
            _ => unreachable!("routed by handle_collector"),
        }
    }

    /// Mirror groups: the preference, a pinned link and a dissolved group.
    async fn collector_mirrors(&mut self, command: CollectorCommand) {
        match command {
            CollectorCommand::SetMirrorPreference { preference, reply } => {
                let result =
                    crate::collector_mirrors::store_preference(&mut self.connection, &preference)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            CollectorCommand::SetMirrorPin { id, pinned, reply } => {
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
            CollectorCommand::DissolveMirrorGroup { id, reply } => {
                let result =
                    crate::collector_mirrors::store_dissolve(&mut self.connection, id).await;
                // Only a dissolve that happened is worth an event; a refusal would make every
                // listener re-read the list to find nothing changed.
                if let Ok((_, Some(event))) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(outcome, _)| outcome));
            }
            _ => unreachable!("routed by handle_collector"),
        }
    }

    /// Handing a package over to the queue.
    async fn collector_enqueue(&mut self, command: CollectorCommand) {
        match command {
            CollectorCommand::ClaimPackageForEnqueue { id, only, reply } => {
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
            CollectorCommand::FinishPackageEnqueue {
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
            _ => unreachable!("routed by handle_collector"),
        }
    }
}

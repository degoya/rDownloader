//! The writer half of `package_store`: package rows, their order and their post-processing state.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::PackagesCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_packages(&mut self, command: PackagesCommand) {
        match command {
            PackagesCommand::CarryEnrichment {
                package_id,
                package_fields,
                files,
                reply,
            } => {
                let result = crate::package_store::carry_enrichment(
                    &mut self.connection,
                    package_id,
                    &package_fields,
                    &files,
                )
                .await;
                send(reply, result);
            }
            PackagesCommand::UpdatePackages { ids, change, reply } => {
                let result =
                    crate::package_store::update_packages(&mut self.connection, &ids, &change)
                        .await;
                publish_config(reply, result, &self.events);
            }
            PackagesCommand::RenamePackageDirectory {
                id,
                name,
                destination,
                reply,
            } => {
                let result = crate::package_store::rename_package_directory(
                    &mut self.connection,
                    id,
                    &name,
                    &destination,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            PackagesCommand::ClearPreviousDestination { id, reply } => {
                send(
                    reply,
                    crate::package_store::clear_previous_destination(&mut self.connection, id)
                        .await,
                );
            }
            PackagesCommand::SwitchPackageDestination {
                id,
                from,
                to,
                reply,
            } => {
                let result = crate::package_relocation_store::switch_package_destination(
                    &mut self.connection,
                    id,
                    &from,
                    &to,
                )
                .await;
                // Only a switch that happened was recorded, so only that one is announced.
                if let Ok((true, event)) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(switched, _)| switched));
            }
            PackagesCommand::ReorderPackages { ids, reply } => {
                let result =
                    crate::package_store::reorder_packages(&mut self.connection, &ids).await;
                publish_unit_event(reply, result, &self.events);
            }
            PackagesCommand::ReorderDownloads {
                package_id,
                ids,
                reply,
            } => {
                let result =
                    crate::package_store::reorder_downloads(&mut self.connection, package_id, &ids)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            PackagesCommand::SetPackageState {
                id,
                state,
                stage,
                percent,
                current,
                reply,
            } => {
                send(
                    reply,
                    self.set_package_state(id, state, stage, percent, current)
                        .await,
                );
            }
            PackagesCommand::SetPackageExtraction { id, result, reply } => {
                send(reply, self.set_package_extraction(id, result).await);
            }
            PackagesCommand::SetPackageSpeedLimit {
                id,
                bytes_per_second,
                reply,
            } => {
                let result = crate::package_speed_limit_store::set_package_speed_limit(
                    &mut self.connection,
                    id,
                    bytes_per_second,
                )
                .await;
                send(reply, result);
            }
            PackagesCommand::SetPackageStartAfter {
                id,
                start_after,
                reply,
            } => {
                let result = crate::package_start_after_store::set_package_start_after(
                    &mut self.connection,
                    id,
                    start_after,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            PackagesCommand::SetPackageDownloadWindow { id, window, reply } => {
                let result = crate::download_window_store::set_package_download_window(
                    &mut self.connection,
                    id,
                    window,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            PackagesCommand::SetStopMark { target, reply } => {
                let result =
                    crate::stop_mark_store::set_stop_mark(&mut self.connection, target).await;
                send(reply, result);
            }
            PackagesCommand::ClearStopMark { only, reply } => {
                let result =
                    crate::stop_mark_store::clear_stop_mark(&mut self.connection, only).await;
                send(reply, result);
            }
        }
    }
}

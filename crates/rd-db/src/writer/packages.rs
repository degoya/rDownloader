//! The writer half of `package_store`: package rows, their order and their post-processing state.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_packages(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::CarryEnrichment {
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
            WriterCommand::UpdatePackages { ids, change, reply } => {
                let result =
                    crate::package_store::update_packages(&mut self.connection, &ids, &change)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::RenamePackageDirectory {
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
            WriterCommand::ClearPreviousDestination { id, reply } => {
                send(
                    reply,
                    crate::package_store::clear_previous_destination(&mut self.connection, id)
                        .await,
                );
            }
            WriterCommand::ReorderPackages { ids, reply } => {
                let result =
                    crate::package_store::reorder_packages(&mut self.connection, &ids).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ReorderDownloads {
                package_id,
                ids,
                reply,
            } => {
                let result =
                    crate::package_store::reorder_downloads(&mut self.connection, package_id, &ids)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetPackageState {
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
            WriterCommand::SetPackageExtraction { id, result, reply } => {
                send(reply, self.set_package_extraction(id, result).await);
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

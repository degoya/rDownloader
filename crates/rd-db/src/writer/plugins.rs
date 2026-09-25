//! Installed artefacts and what they are trusted to do: the writer half of
//! `plugin_transfer_store`, `plugin_execution_store`, `plugin_keys_store`,
//! `plugin_revocations_store`, `remote_job_store` and `managed_tools_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_plugins(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::SavePluginTransfer {
                id,
                plugin_id,
                plugin_version,
                checkpoint,
                reply,
            } => {
                send(
                    reply,
                    crate::plugin_transfer_store::save_plugin_transfer(
                        &mut self.connection,
                        id,
                        &plugin_id,
                        &plugin_version,
                        checkpoint,
                    )
                    .await,
                );
            }
            WriterCommand::ClearPluginTransfer { id, reply } => {
                send(
                    reply,
                    crate::plugin_transfer_store::clear_plugin_transfer(&mut self.connection, id)
                        .await,
                );
            }
            WriterCommand::RecordPluginExecution { entry, reply } => {
                send(
                    reply,
                    crate::plugin_execution_store::record_plugin_execution(
                        &mut self.connection,
                        *entry,
                    )
                    .await,
                );
            }
            WriterCommand::TrustPluginKey { input, reply } => {
                let result = crate::plugin_keys_store::insert_plugin_trusted_key(
                    &mut self.connection,
                    input,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::RevokePluginKey { key_id, reply } => {
                let result = crate::plugin_keys_store::delete_plugin_trusted_key(
                    &mut self.connection,
                    &key_id,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::RevokePluginDigest { input, reply } => {
                let result = crate::plugin_revocations_store::insert_plugin_digest_revocation(
                    &mut self.connection,
                    input,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UnrevokePluginDigest { digest, reply } => {
                let result = crate::plugin_revocations_store::delete_plugin_digest_revocation(
                    &mut self.connection,
                    &digest,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::ClaimRemoteJob { input, reply } => {
                let result = crate::remote_job_store::claim(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::AdvanceRemoteJob { id, input, reply } => {
                let result =
                    crate::remote_job_store::advance(&mut self.connection, id, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteRemoteJob { id, reply } => {
                let result = crate::remote_job_store::remove(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::RecordManagedTool { input, reply } => {
                let result =
                    crate::managed_tools_store::record_managed_tool(&mut self.connection, input)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::ForgetManagedTool {
                name,
                version,
                reply,
            } => {
                let result = crate::managed_tools_store::forget_managed_tool(
                    &mut self.connection,
                    &name,
                    &version,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::AcceptToolManifest {
                sequence,
                issued_at,
                reply,
            } => {
                let result = crate::managed_tools_store::accept_tool_manifest(
                    &mut self.connection,
                    sequence,
                    &issued_at,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
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

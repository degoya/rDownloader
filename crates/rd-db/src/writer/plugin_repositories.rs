//! The writer half of `plugin_repositories_store` (RD-140-01).

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the plugin repository commands.
    pub(super) async fn handle_plugin_repositories(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::AddPluginRepository { input, reply } => {
                let result = crate::plugin_repositories_store::insert_plugin_repository(
                    &mut self.connection,
                    input,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdatePluginRepository {
                id,
                enabled,
                name,
                reply,
            } => {
                let result = crate::plugin_repositories_store::update_plugin_repository(
                    &mut self.connection,
                    &id,
                    enabled,
                    name,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeletePluginRepository { id, reply } => {
                let result = crate::plugin_repositories_store::delete_plugin_repository(
                    &mut self.connection,
                    &id,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::RecordPluginRepositoryCheck { id, check, reply } => {
                let result = crate::plugin_repositories_store::record_plugin_repository_check(
                    &mut self.connection,
                    &id,
                    check,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::WithdrawPluginKey { input, reply } => {
                let result = crate::plugin_repositories_store::withdraw_plugin_key(
                    &mut self.connection,
                    input,
                )
                .await;
                // Announced only when the key was not withdrawn before: every refresh repeats
                // the index's list, and a repeat changes nothing anybody has to re-read.
                match result {
                    Ok((true, event)) => publish_config(reply, Ok((true, event)), &self.events),
                    Ok((false, _)) => send(reply, Ok(false)),
                    Err(error) => send(reply, Err(error)),
                }
            }
            WriterCommand::RecordPluginRepositoryInstall { input, reply } => {
                let result = crate::plugin_repositories_store::record_plugin_repository_install(
                    &mut self.connection,
                    input,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            // Routed here by `Writer::run` only for the variants above; see `handle_plugins`.
            _ => {}
        }
    }
}

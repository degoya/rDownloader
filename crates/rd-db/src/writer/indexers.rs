//! The writer half of `indexer_store` (RD-180-19).

use super::{Writer, publish_config};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the indexer commands.
    pub(super) async fn handle_indexers(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::CreateIndexer { input, reply } => {
                let result = crate::indexer_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateIndexer { id, input, reply } => {
                let result = crate::indexer_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(indexer, orphan, event)| ((indexer, orphan), event));
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteIndexer { id, reply } => {
                let result = crate::indexer_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            // Routed here by `Writer::run` only for the variants above; see `handle_plugins`.
            _ => {}
        }
    }
}

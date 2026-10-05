//! The writer half of `indexer_store` (RD-180-19).

use super::{Writer, publish_config};
use crate::commands::IndexersCommand;

impl Writer {
    /// Applies the indexer commands.
    pub(super) async fn handle_indexers(&mut self, command: IndexersCommand) {
        match command {
            IndexersCommand::CreateIndexer { input, reply } => {
                let result = crate::indexer_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            IndexersCommand::UpdateIndexer { id, input, reply } => {
                let result = crate::indexer_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(indexer, orphan, event)| ((indexer, orphan), event));
                publish_config(reply, result, &self.events);
            }
            IndexersCommand::DeleteIndexer { id, reply } => {
                let result = crate::indexer_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
        }
    }
}

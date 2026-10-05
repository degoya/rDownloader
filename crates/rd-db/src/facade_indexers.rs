//! Database facade methods for the Newznab indexers a person defines once (RD-180-19).

use anyhow::Result;
use rd_core::{Indexer, IndexerId};

use crate::{
    Database,
    commands::IndexersCommand,
    indexer_store::{self, NewIndexer},
    writer,
};

impl Database {
    /// Every indexer, switched off ones included, by name.
    pub async fn list_indexers(&self) -> Result<Vec<Indexer>> {
        indexer_store::list(&self.readers).await
    }

    pub async fn indexer(&self, id: IndexerId) -> Result<Option<Indexer>> {
        indexer_store::get(&self.readers, id).await
    }

    pub async fn create_indexer(&self, input: NewIndexer) -> Result<Indexer> {
        writer::request(&self.writer, |reply| IndexersCommand::CreateIndexer {
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// Updates an indexer and returns it with the key reference the edit replaced, if any.
    pub async fn update_indexer(
        &self,
        id: IndexerId,
        input: NewIndexer,
    ) -> Result<(Indexer, Option<String>)> {
        writer::request(&self.writer, |reply| IndexersCommand::UpdateIndexer {
            id,
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// Deletes an indexer; returns its key reference for the vault.
    pub async fn delete_indexer(&self, id: IndexerId) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| IndexersCommand::DeleteIndexer {
            id,
            reply,
        })
        .await
    }
}

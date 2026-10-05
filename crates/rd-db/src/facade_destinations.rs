//! Database facade for storage roots, categories, category rules and hot folders.

use anyhow::Result;

use crate::{
    Database, NewCategory, NewCategoryRule, NewHotFolder, NewStorageRoot, commands::ConfigCommand,
    config_store, writer,
};

impl Database {
    /// Adds an allowlisted download destination.
    /// Creates a root under the id the caller already materialised the directory with.
    pub async fn create_storage_root(
        &self,
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
    ) -> Result<rd_core::StorageRootConfig> {
        writer::request(&self.writer, |reply| ConfigCommand::CreateStorageRoot {
            id,
            input,
            reply,
        })
        .await
    }

    /// Lists all allowlisted download destinations.
    pub async fn list_storage_roots(&self) -> Result<Vec<rd_core::StorageRootConfig>> {
        config_store::list_storage_roots(&self.readers).await
    }

    /// The storage root new downloads land on when no category picks one.
    pub async fn default_storage_root(&self) -> Result<Option<rd_core::StorageRootConfig>> {
        config_store::default_storage_root(&self.readers).await
    }

    /// Adds a category mapped to one storage root and relative destination.
    pub async fn create_category(&self, input: NewCategory) -> Result<rd_core::Category> {
        writer::request(&self.writer, |reply| ConfigCommand::CreateCategory {
            input,
            reply,
        })
        .await
    }

    /// Lists categories in display order.
    pub async fn list_categories(&self) -> Result<Vec<rd_core::Category>> {
        config_store::list_categories(&self.readers).await
    }

    /// Adds one prioritized first-match category rule.
    pub async fn create_category_rule(
        &self,
        input: NewCategoryRule,
    ) -> Result<rd_core::CategoryRule> {
        writer::request(&self.writer, |reply| ConfigCommand::CreateCategoryRule {
            input,
            reply,
        })
        .await
    }

    /// Lists category rules by priority.
    pub async fn list_category_rules(&self) -> Result<Vec<rd_core::CategoryRule>> {
        config_store::list_category_rules(&self.readers).await
    }

    /// Persists one daemon or capture-agent hotfolder.
    pub async fn create_hotfolder(&self, input: NewHotFolder) -> Result<rd_core::HotFolderConfig> {
        writer::request(&self.writer, |reply| ConfigCommand::CreateHotFolder {
            input,
            reply,
        })
        .await
    }

    /// Lists configured daemon and capture-agent hotfolders.
    pub async fn list_hotfolders(&self) -> Result<Vec<rd_core::HotFolderConfig>> {
        config_store::list_hotfolders(&self.readers).await
    }

    /// Replaces a storage root's name, path and default flag.
    pub async fn update_storage_root(
        &self,
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
    ) -> Result<rd_core::StorageRootConfig> {
        writer::request(&self.writer, |reply| ConfigCommand::UpdateStorageRoot {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a storage root; fails while categories still point at it.
    pub async fn delete_storage_root(&self, id: rd_core::StorageRootId) -> Result<()> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteStorageRoot {
            id,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a category.
    pub async fn update_category(
        &self,
        id: rd_core::CategoryId,
        input: NewCategory,
    ) -> Result<rd_core::Category> {
        writer::request(&self.writer, |reply| ConfigCommand::UpdateCategory {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a category and its rules; fails while unfinished packages use it.
    pub async fn delete_category(&self, id: rd_core::CategoryId) -> Result<()> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteCategory {
            id,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a category rule.
    pub async fn update_category_rule(
        &self,
        id: rd_core::CategoryRuleId,
        input: NewCategoryRule,
    ) -> Result<rd_core::CategoryRule> {
        writer::request(&self.writer, |reply| ConfigCommand::UpdateCategoryRule {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes one category rule.
    pub async fn delete_category_rule(&self, id: rd_core::CategoryRuleId) -> Result<()> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteCategoryRule {
            id,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a hotfolder.
    pub async fn update_hotfolder(
        &self,
        id: rd_core::HotFolderId,
        input: NewHotFolder,
    ) -> Result<rd_core::HotFolderConfig> {
        writer::request(&self.writer, |reply| ConfigCommand::UpdateHotFolder {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes one hotfolder; the caller stops its watcher.
    pub async fn delete_hotfolder(&self, id: rd_core::HotFolderId) -> Result<()> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteHotFolder {
            id,
            reply,
        })
        .await
    }
}

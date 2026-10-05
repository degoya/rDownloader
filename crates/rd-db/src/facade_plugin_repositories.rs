//! Database facade methods for the plugin repositories (RD-140-01).

use anyhow::Result;

use crate::{
    Database,
    commands::PluginRepositoriesCommand,
    plugin_repositories_store::{
        self, NewPluginRepository, PluginRepository, PluginRepositoryInstall, PluginWithdrawnKey,
        RepositoryCheck,
    },
    writer,
};

impl Database {
    /// Every configured repository, the official one first.
    pub async fn list_plugin_repositories(&self) -> Result<Vec<PluginRepository>> {
        plugin_repositories_store::list_plugin_repositories(&self.readers).await
    }

    /// One repository by id.
    pub async fn plugin_repository(&self, id: &str) -> Result<Option<PluginRepository>> {
        plugin_repositories_store::plugin_repository(&self.readers, id).await
    }

    /// Every plugin signing key a repository withdrew, read once at start into the verifier.
    pub async fn list_plugin_withdrawn_keys(&self) -> Result<Vec<PluginWithdrawnKey>> {
        plugin_repositories_store::list_plugin_withdrawn_keys(&self.readers).await
    }

    /// Which installed versions came from which repository.
    pub async fn list_plugin_repository_installs(&self) -> Result<Vec<PluginRepositoryInstall>> {
        plugin_repositories_store::list_plugin_repository_installs(&self.readers).await
    }

    /// Records a third-party repository whose key the person approved.
    pub async fn add_plugin_repository(
        &self,
        input: NewPluginRepository,
    ) -> Result<PluginRepository> {
        writer::request(&self.writer, |reply| {
            PluginRepositoriesCommand::AddPluginRepository { input, reply }
        })
        .await
    }

    /// Switches a repository on or off and renames it; returns whether it exists.
    pub async fn update_plugin_repository(
        &self,
        id: String,
        enabled: Option<bool>,
        name: Option<String>,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            PluginRepositoriesCommand::UpdatePluginRepository {
                id,
                enabled,
                name,
                reply,
            }
        })
        .await
    }

    /// Removes a third-party repository; returns whether one was removed.
    pub async fn delete_plugin_repository(&self, id: String) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            PluginRepositoriesCommand::DeletePluginRepository { id, reply }
        })
        .await
    }

    /// Records how a refresh of one repository ended, raising its replay floor on success.
    pub async fn record_plugin_repository_check(
        &self,
        id: String,
        check: RepositoryCheck,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            PluginRepositoriesCommand::RecordPluginRepositoryCheck { id, check, reply }
        })
        .await
    }

    /// Records a withdrawn plugin signing key and drops a trusted key it names; returns whether
    /// the key was not withdrawn before.
    pub async fn withdraw_plugin_key(&self, input: PluginWithdrawnKey) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            PluginRepositoriesCommand::WithdrawPluginKey { input, reply }
        })
        .await
    }

    /// Records which repository an installed version came from.
    pub async fn record_plugin_repository_install(
        &self,
        input: PluginRepositoryInstall,
    ) -> Result<PluginRepositoryInstall> {
        writer::request(&self.writer, |reply| {
            PluginRepositoriesCommand::RecordPluginRepositoryInstall { input, reply }
        })
        .await
    }
}

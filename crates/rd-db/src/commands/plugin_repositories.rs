//! The commands of `writer/plugin_repositories.rs`.

use super::Reply;

/// The commands `Writer::handle_plugin_repositories` applies.
pub(crate) enum PluginRepositoriesCommand {
    /// Records a third-party plugin repository whose key was approved (RD-140-01).
    AddPluginRepository {
        input: crate::NewPluginRepository,
        reply: Reply<crate::PluginRepository>,
    },
    UpdatePluginRepository {
        id: String,
        enabled: Option<bool>,
        name: Option<String>,
        reply: Reply<bool>,
    },
    DeletePluginRepository {
        id: String,
        reply: Reply<bool>,
    },
    /// Raises a repository's replay floor after an index verified, or records why it did not.
    RecordPluginRepositoryCheck {
        id: String,
        check: crate::RepositoryCheck,
        reply: Reply<()>,
    },
    /// Records a plugin signing key a repository index withdrew.
    WithdrawPluginKey {
        input: crate::PluginWithdrawnKey,
        reply: Reply<bool>,
    },
    RecordPluginRepositoryInstall {
        input: crate::PluginRepositoryInstall,
        reply: Reply<crate::PluginRepositoryInstall>,
    },
}

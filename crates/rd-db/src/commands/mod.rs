//! What the facade sends the serialized writer: one enum per module under `writer/`, each
//! wrapped in one variant of [`WriterCommand`].

use anyhow::Result;
use tokio::sync::oneshot;

mod archive_passwords;
mod audit;
mod auth;
mod bandwidth;
mod collector;
mod config;
mod downloads;
mod full_backup;
mod history;
mod indexers;
mod logs;
mod maintenance;
mod network;
mod notify;
mod nzb;
mod object_storage;
mod packages;
mod plugin_repositories;
mod plugins;
mod sessions;
mod sources;
mod storage;
mod streams;
mod subscriptions;

pub(crate) use archive_passwords::ArchivePasswordsCommand;
pub(crate) use audit::AuditCommand;
pub(crate) use auth::AuthCommand;
pub(crate) use bandwidth::BandwidthCommand;
pub(crate) use collector::CollectorCommand;
pub(crate) use config::ConfigCommand;
pub(crate) use downloads::DownloadsCommand;
pub(crate) use full_backup::{BackupLedgerCommand, FullBackupCommand};
pub(crate) use history::HistoryCommand;
pub(crate) use indexers::IndexersCommand;
pub(crate) use logs::LogsCommand;
pub(crate) use maintenance::MaintenanceCommand;
pub(crate) use network::NetworkCommand;
pub(crate) use notify::NotifyCommand;
pub(crate) use nzb::NzbCommand;
pub(crate) use object_storage::ObjectStorageCommand;
pub(crate) use packages::PackagesCommand;
pub(crate) use plugin_repositories::PluginRepositoriesCommand;
pub(crate) use plugins::PluginsCommand;
pub(crate) use sessions::SessionsCommand;
pub(crate) use sources::SourcesCommand;
pub(crate) use storage::StorageCommand;
pub(crate) use streams::StreamsCommand;
pub(crate) use subscriptions::SubscriptionsCommand;

pub(crate) type Reply<T> = oneshot::Sender<Result<T>>;

/// What deleting an account releases: its own secret and sign-in references, and the ones its
/// sign-in held.
pub(crate) type ReleasedAccountRefs = ((Option<String>, Option<String>), Vec<String>);

// Every message is as large as the largest command, as it was with one flat enum.
#[allow(clippy::large_enum_variant)]
pub(crate) enum WriterCommand {
    /// Closes the writer's connection and ends its task; every later command fails.
    Close {
        reply: Reply<()>,
    },
    Downloads(DownloadsCommand),
    Sources(SourcesCommand),
    Packages(PackagesCommand),
    Collector(CollectorCommand),
    Nzb(NzbCommand),
    Config(ConfigCommand),
    Network(NetworkCommand),
    Auth(AuthCommand),
    Sessions(SessionsCommand),
    Plugins(PluginsCommand),
    PluginRepositories(PluginRepositoriesCommand),
    ObjectStorage(ObjectStorageCommand),
    Indexers(IndexersCommand),
    Streams(StreamsCommand),
    Subscriptions(SubscriptionsCommand),
    Notify(NotifyCommand),
    Bandwidth(BandwidthCommand),
    Maintenance(MaintenanceCommand),
    ArchivePasswords(ArchivePasswordsCommand),
    FullBackup(FullBackupCommand),
    BackupLedger(BackupLedgerCommand),
    Logs(LogsCommand),
    Audit(AuditCommand),
    Storage(StorageCommand),
    History(HistoryCommand),
}

/// Lets a facade method hand `writer::request` the command of its area as it is.
macro_rules! wrap_area_commands {
    ($($variant:ident($command:ident)),* $(,)?) => {
        $(
            impl From<$command> for WriterCommand {
                fn from(command: $command) -> Self {
                    Self::$variant(command)
                }
            }
        )*
    };
}

wrap_area_commands! {
    Downloads(DownloadsCommand),
    Sources(SourcesCommand),
    Packages(PackagesCommand),
    Collector(CollectorCommand),
    Nzb(NzbCommand),
    Config(ConfigCommand),
    Network(NetworkCommand),
    Auth(AuthCommand),
    Sessions(SessionsCommand),
    Plugins(PluginsCommand),
    PluginRepositories(PluginRepositoriesCommand),
    ObjectStorage(ObjectStorageCommand),
    Indexers(IndexersCommand),
    Streams(StreamsCommand),
    Subscriptions(SubscriptionsCommand),
    Notify(NotifyCommand),
    Bandwidth(BandwidthCommand),
    Maintenance(MaintenanceCommand),
    ArchivePasswords(ArchivePasswordsCommand),
    FullBackup(FullBackupCommand),
    BackupLedger(BackupLedgerCommand),
    Logs(LogsCommand),
    Audit(AuditCommand),
    Storage(StorageCommand),
    History(HistoryCommand),
}

//! The services the [`AppState`](crate::AppState) is built from that need more than one line:
//! the remote transports, the subscription adapters, the managed tool store and the plugin
//! repositories.

use rd_db::Database;
use rd_scheduler::SchedulerHandle;

use crate::{RemoteServices, subscription_service};

impl RemoteServices {
    /// Builds the transports on the scheduler's network defaults, so a proxy and a custom
    /// CA reach them exactly as they reach HTTP. Byte pacing is not passed here: the queue
    /// hands each runner a limiter scoped to the transfer it is about to start.
    #[must_use]
    pub fn new(
        database: Database,
        secrets: rd_secrets::SecretStore,
        settings: rd_ftp::SharedRemoteSettings,
        network_defaults: rd_http::SharedNetworkDefaults,
    ) -> Self {
        Self {
            ftp: rd_ftp::FtpService::new(
                database.clone(),
                secrets.clone(),
                settings.clone(),
                network_defaults.clone(),
            ),
            object_storage: rd_object_storage::ObjectStorageService::new(
                database.clone(),
                secrets.clone(),
                settings.clone(),
                network_defaults,
            ),
            sftp: rd_sftp::SftpService::new(database, secrets, settings.clone()),
            settings,
        }
    }
}

/// The sources a subscription can watch.
///
/// The media adapter reuses the existing probe rather than talking to yt-dlp a second
/// way; RD-080-10, RD-080-11, RD-110-21, RD-130-19 and RD-190-13 add their adapters to
/// this same list.
pub(super) fn subscription_adapters(
    database: &Database,
    media_probe: &std::sync::Arc<dyn rd_media::MediaProbe>,
    scheduler: &SchedulerHandle,
    secrets: &rd_secrets::SecretStore,
    extraction: &rd_extract::ExtractionService,
    site_rule_claims: &subscription_service::SharedSiteRules,
) -> Vec<std::sync::Arc<dyn rd_subscription::SourceAdapter>> {
    vec![
        std::sync::Arc::new(rd_subscription::MediaAdapter::new(media_probe.clone())),
        std::sync::Arc::new(rd_subscription::FeedAdapter::new(std::sync::Arc::new(
            subscription_service::HttpFeedFetcher::new(scheduler.clone()),
        ))),
        std::sync::Arc::new(rd_subscription::IndexerAdapter::new(
            std::sync::Arc::new(subscription_service::HttpFeedFetcher::new(
                scheduler.clone(),
            )),
            std::sync::Arc::new(subscription_service::VaultSecretResolver::new(
                secrets.clone(),
            )),
            // Where a poll meets what it already has, so it pages no further (RD-1150-05).
            std::sync::Arc::new(subscription_service::ArchivedItems::new(database.clone())),
        )),
        // A watched release or series page (RD-110-21): the listing is fetched
        // conditionally like a feed, and which of its links are release pages is the
        // rules' answer rather than a guess.
        std::sync::Arc::new(rd_subscription::RuleAdapter::new(
            std::sync::Arc::new(subscription_service::HttpFeedFetcher::new(
                scheduler.clone(),
            )),
            std::sync::Arc::new(site_rule_claims.clone()),
        )),
        // An administrator's script whose output lines are links (RD-130-19), run through
        // the same sandbox as every other user script.
        std::sync::Arc::new(rd_subscription::ScriptAdapter::new(std::sync::Arc::new(
            subscription_service::SandboxScriptRunner::new(extraction.clone()),
        ))),
        // The releases of a GitHub or GitLab repository (RD-190-13), read through the
        // forge's API with the subscription's token from the vault, when it has one.
        std::sync::Arc::new(rd_subscription::GitReleaseAdapter::new(
            std::sync::Arc::new(subscription_service::HttpApiFetcher::new(scheduler.clone())),
            std::sync::Arc::new(subscription_service::VaultSecretResolver::new(
                secrets.clone(),
            )),
        )),
    ]
}

/// The managed tool store.
///
/// Rooted at the data directory when the binary registered one. `data/` is the same
/// default the CLI uses, so a test that never sets one reads an empty store rather
/// than an absolute path from somewhere else.
pub(super) fn managed_tools(database: &Database) -> rd_tools::ManagedToolService {
    rd_tools::ManagedToolService::new(
        database.clone(),
        rd_tools::store_root(
            rd_core::data_directory().unwrap_or_else(|| std::path::Path::new("data")),
        ),
        rd_core::ManagedToolSettings::default(),
    )
}

/// The plugin repositories, kept beside the data directory.
pub(super) fn plugin_repositories(
    database: &Database,
    plugins: &rd_plugin_host::PluginInstaller,
) -> rd_plugin_host::repository::PluginRepositoryService {
    rd_plugin_host::repository::PluginRepositoryService::new(
        database.clone(),
        rd_core::data_directory()
            .unwrap_or_else(|| std::path::Path::new("data"))
            .join(rd_plugin_host::repository::DIRECTORY),
        plugins.clone(),
    )
}

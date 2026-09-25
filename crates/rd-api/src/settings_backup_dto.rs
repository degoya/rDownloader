use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{dto::SettingsResponse, settings_backup_crypto::EncryptedSecrets};

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAccount {
    pub id: rd_core::AccountId,
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential the secret slot holds, for a provider that offers a choice. Restoring
    /// without it would silently move the account to its provider's default mode, i.e. change
    /// which host the stored credential is allowed to reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub enabled: bool,
    #[serde(default)]
    pub secret_slot: Option<String>,
    #[serde(default)]
    pub cookies_slot: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleProxyProfile {
    pub id: rd_core::ProxyProfileId,
    pub name: String,
    pub kind: rd_core::ProxyKind,
    #[schema(value_type = String, format = "uri")]
    pub endpoint: Url,
    pub username: Option<String>,
    #[serde(default)]
    pub secret_slot: Option<String>,
}

/// Auth profile in a settings bundle. Credentials travel as slot names, never as values.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAuthProfile {
    pub id: rd_core::AuthProfileId,
    pub name: String,
    #[serde(flatten)]
    pub scope: rd_core::AuthScope,
    pub method: rd_core::AuthMethod,
    pub origin: rd_core::AuthOrigin,
    pub enabled: bool,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub secret_slot: Option<String>,
    #[serde(default)]
    pub certificate_slot: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleUsenetServer {
    pub id: rd_core::UsenetServerId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
    #[serde(default)]
    pub password_slot: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleStreamChannel {
    pub id: rd_core::StreamChannelId,
    pub url: String,
    pub name: String,
    pub quality: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    pub enabled: bool,
    #[serde(default)]
    pub recording: rd_core::RecordingPolicy,
}

/// A subscription in a settings bundle (RD-080-07).
///
/// The item archive is deliberately absent: history is not configuration, and telling
/// another machine it has already downloaded things it has not is the one mistake here that
/// silently loses content. A restored subscription is unprimed and applies its backlog
/// policy again, which defaults to ignoring everything that already exists.
#[derive(Clone, Deserialize, Serialize, ToSchema)]
pub struct BundleSubscription {
    pub id: rd_core::SubscriptionId,
    pub name: String,
    pub url: String,
    pub kind: rd_core::SubscriptionKind,
    pub enabled: bool,
    pub mode: rd_core::SubscriptionMode,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub priority: rd_core::DownloadPriority,
    pub interval_seconds: u32,
    #[serde(default)]
    pub filters: rd_core::SubscriptionFilters,
    #[serde(default)]
    pub backlog: rd_core::BacklogPolicy,
    /// Where an indexer's own categories are routed, and which of them are asked for.
    ///
    /// Both were absent until 1.0.1, so restoring a bundle silently dropped an indexer's routing
    /// and left it pulling the whole feed. Defaulted, so an older bundle still restores.
    #[serde(default)]
    pub category_map: Vec<rd_core::CategoryMapping>,
    #[serde(default)]
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21). Absent from
    /// a bundle written before it existed, which restores as the default it always had.
    #[serde(default)]
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits (RD-120-37). Absent from a bundle written
    /// before it existed, which restores as the list every subscription showed then.
    #[serde(default)]
    pub view: rd_core::SubscriptionView,
    /// Whether the card slider turns its pages on its own (RD-120-37); off when absent.
    #[serde(default)]
    pub autoplay: bool,
    /// The shape of a card's image area (RD-120-42). Absent from a bundle written before it
    /// existed, which restores as the `2:1` every card had then.
    #[serde(default)]
    pub card_ratio: rd_core::SubscriptionCardRatio,
    /// The cron expression replacing the interval (RD-130-19). Absent from a bundle written
    /// before it existed, which restores the interval every subscription had then. A script
    /// subscription travels here, unlike in an area bundle: this is the administrator's whole
    /// configuration, restored only by the administrator.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Slot of the indexer API key inside the encrypted section, if the subscription has one.
    #[serde(default)]
    pub secret_slot: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, ToSchema)]
pub struct SettingsBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub app_version: String,
    pub settings: SettingsResponse,
    #[serde(default)]
    pub storage_roots: Vec<rd_core::StorageRootConfig>,
    #[serde(default)]
    pub categories: Vec<rd_core::Category>,
    #[serde(default)]
    pub category_rules: Vec<rd_core::CategoryRule>,
    #[serde(default)]
    pub hotfolders: Vec<rd_core::HotFolderConfig>,
    #[serde(default)]
    pub stream_channels: Vec<BundleStreamChannel>,
    #[serde(default)]
    pub subscriptions: Vec<BundleSubscription>,
    #[serde(default)]
    pub proxy_profiles: Vec<BundleProxyProfile>,
    #[serde(default)]
    pub accounts: Vec<BundleAccount>,
    #[serde(default)]
    pub usenet_servers: Vec<BundleUsenetServer>,
    #[serde(default)]
    pub auth_profiles: Vec<BundleAuthProfile>,
    #[serde(default)]
    pub secrets: Option<EncryptedSecrets>,
}

#[derive(Deserialize, ToSchema)]
pub struct ExportSettingsRequest {
    #[schema(write_only)]
    pub passphrase: Option<String>,
    pub include_secrets: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ImportSettingsRequest {
    pub bundle: SettingsBundle,
    #[schema(write_only)]
    pub passphrase: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ImportSummaryResponse {
    pub settings: usize,
    pub storage_roots: usize,
    pub categories: usize,
    pub category_rules: usize,
    pub hotfolders: usize,
    pub stream_channels: usize,
    pub subscriptions: usize,
    pub proxy_profiles: usize,
    pub accounts: usize,
    pub usenet_servers: usize,
}

impl ImportSummaryResponse {
    pub(crate) fn from_bundle(bundle: &SettingsBundle) -> Self {
        Self {
            settings: 1,
            storage_roots: bundle.storage_roots.len(),
            categories: bundle.categories.len(),
            category_rules: bundle.category_rules.len(),
            hotfolders: bundle.hotfolders.len(),
            stream_channels: bundle.stream_channels.len(),
            subscriptions: bundle.subscriptions.len(),
            proxy_profiles: bundle.proxy_profiles.len(),
            accounts: bundle.accounts.len(),
            usenet_servers: bundle.usenet_servers.len(),
        }
    }
}

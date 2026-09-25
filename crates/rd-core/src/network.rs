use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{AccountId, PluginId, ProxyProfileId};

/// Public account metadata. Secret values remain in the secret store.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Account {
    pub id: AccountId,
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential the account's secret slot holds, for a provider that offers a choice
    /// (`ddownload`). `None` for every provider with only one way to sign in, and for accounts
    /// written before their provider grew a second mode — those fall back to the provider's
    /// first declared mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub enabled: bool,
    pub has_secret: bool,
    pub has_cookies: bool,
}

/// Supported proxy transports.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    Http,
    Https,
    Socks5,
}

/// User-configured network proxy without plaintext credentials.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ProxyProfile {
    pub id: ProxyProfileId,
    pub name: String,
    pub kind: ProxyKind,
    #[schema(value_type = String, format = "uri")]
    pub endpoint: Url,
    pub username: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub secret_ref: Option<String>,
    pub has_credentials: bool,
}

/// Resolver selected for a link.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ResolverRoute {
    pub plugin_id: Option<PluginId>,
    pub account_id: Option<AccountId>,
    pub label: String,
    pub priority: i32,
}

/// Exact resolver version pinned to one persistent download job.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ResolverPin {
    pub plugin_id: PluginId,
    pub version: String,
}

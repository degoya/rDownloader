//! Provider accounts, proxy profiles and Usenet servers.

use super::*;

/// Redaction-safe result of a live provider account check.
#[derive(Serialize, ToSchema)]
pub struct AccountTestResponse {
    pub valid: bool,
    pub premium: bool,
    /// What the interface prints next to the account, one translated part after another,
    /// joined with a separator of its own. Empty when the check has nothing to add to the flags.
    pub label: Vec<AccountLabelPart>,
    pub traffic_left: Option<rd_core::ByteCount>,
}

/// One translatable part of an account label, in the shape of every coded server message:
/// `code` is looked up in the active language, then in English, `params` are interpolated,
/// and `message` is printed only when no catalogue knows the code.
#[derive(Serialize, ToSchema)]
pub struct AccountLabelPart {
    /// `plugin.account.*` from the core catalogue or `<provider.slug>.*` from the plugin's own.
    pub code: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: std::collections::BTreeMap<String, String>,
    /// English, redaction-safe text for a code no catalogue translates.
    pub message: String,
}

impl From<rd_plugin_host::LabelPart> for AccountLabelPart {
    fn from(part: rd_plugin_host::LabelPart) -> Self {
        Self {
            code: part.code,
            params: part.params,
            message: part.message,
        }
    }
}

/// Account metadata with write-only secret material.
#[derive(Deserialize, ToSchema)]
pub struct CreateAccountRequest {
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential `secret` holds, for a provider that offers a choice. Required for
    /// those providers, rejected for every other one.
    #[serde(default)]
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub cookies: Option<String>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub enabled: bool,
}

/// Editable account metadata. Empty secret fields preserve their stored values.
#[derive(Deserialize, ToSchema)]
pub struct UpdateAccountRequest {
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential `secret` holds, for a provider that offers a choice. Required for
    /// those providers, rejected for every other one.
    #[serde(default)]
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub cookies: Option<String>,
    pub clear_secret: bool,
    pub clear_cookies: bool,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub enabled: bool,
}

/// Network proxy profile with a write-only password.
#[derive(Deserialize, ToSchema)]
pub struct CreateProxyProfileRequest {
    pub name: String,
    pub kind: rd_core::ProxyKind,
    #[schema(format = "uri")]
    pub endpoint: String,
    pub username: Option<String>,
    #[schema(write_only)]
    pub password: Option<String>,
}

/// Persistent NNTP endpoint with a write-only password.
#[derive(Deserialize, ToSchema)]
pub struct CreateUsenetServerRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    #[schema(write_only)]
    pub password: Option<String>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

/// Editable NNTP endpoint. An omitted password preserves the stored password.
#[derive(Deserialize, ToSchema)]
pub struct UpdateUsenetServerRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    #[schema(write_only)]
    pub password: Option<String>,
    pub clear_password: bool,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

/// Body of `PUT /api/v1/usenet/servers/{id}/quota` (RD-1100-05).
#[derive(Deserialize, ToSchema)]
pub struct SetUsenetQuotaRequest {
    /// The bytes the server may deliver; absent or `null` removes the quota.
    #[serde(default)]
    pub limit_bytes: Option<u64>,
    /// What happens once the limit is reached: `backup` (asked only after every other server)
    /// or `pause` (not asked at all).
    #[serde(default)]
    pub action: rd_core::UsenetQuotaAction,
    /// The day (UTC) from which the used figure starts again at zero, once. Today or later.
    #[serde(default)]
    pub reset_on: Option<chrono::NaiveDate>,
    /// Puts the used figure back to zero now.
    #[serde(default)]
    pub reset_usage: bool,
}

/// Hoster domains an account's provider can download from.
#[derive(Serialize, ToSchema)]
pub struct AccountHostersResponse {
    pub account_id: rd_core::AccountId,
    pub provider: String,
    pub hosters: Vec<String>,
}

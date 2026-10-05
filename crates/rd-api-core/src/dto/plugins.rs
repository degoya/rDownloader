//! Installed plugins, their executions and trust, and the provider table.

use super::*;

/// Redaction-safe metadata for one installed resolver version.
///
/// `name` and `description` are the manifest's own values; the plugin manager overlays the
/// localised ones from `/api/v1/plugins/i18n/{locale}` when they exist.
#[derive(Serialize, ToSchema)]
pub struct InstalledPluginResponse {
    /// Whether this is the version new work of its id runs on right now.
    ///
    /// Installing never removes an older version, so two can sit side by side; without a
    /// version choice the highest SemVer is the one loaded, and with one (RD-140-02) the chosen
    /// version is. That rule was never wrong, only invisible — the manager listed both with
    /// nothing to separate them, so the leftover looked like a second, equal plugin.
    ///
    /// Says nothing about whether the plugin is switched on; that is a separate choice and
    /// applies to every version of an id at once.
    #[serde(default)]
    pub active: bool,
    /// How many invocations this plugin id has recorded, capped at what the store keeps.
    ///
    /// Only the number, never an entry: the manager offers its diagnostics accordion when this
    /// is greater than zero and fetches the entries themselves when somebody opens it, which is
    /// what keeps diagnostics nobody looks at free. Without it the accordion was a button with
    /// nothing behind it, and the panel it opened could not say whether the plugin had never
    /// run or run without incident.
    ///
    /// Counted per id, so every installed version of one id reports the same number — the
    /// store records the version in the entry, not in its key.
    #[serde(default)]
    pub execution_count: u32,
    pub id: rd_core::PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub homepage: Option<String>,
    pub support_url: Option<String>,
    pub license: Option<String>,
    /// Provider slug this resolver serves.
    pub provider_slug: String,
    /// What the plugin is: `resolver`, `transfer`, or whatever an unknown package claims.
    pub plugin_type: String,
    /// `rdownloader:plugin` WIT version the package was built against.
    pub api_version: String,
    /// Every grant the manifest asks for, as the plugin manager lists them.
    pub capabilities: Vec<String>,
    pub domains: Vec<String>,
    pub max_concurrent_downloads: u32,
}

impl From<rd_plugin_host::PluginManifest> for InstalledPluginResponse {
    fn from(manifest: rd_plugin_host::PluginManifest) -> Self {
        let domains = manifest.domains().to_vec();
        let capabilities = manifest.capabilities.granted();
        let provider_slug = manifest.message_slug().to_owned();
        Self {
            // Decided by the caller, which sees the whole list; one manifest cannot know
            // whether a higher version of itself is installed alongside it.
            active: false,
            // Likewise: the count comes from the execution store, which a manifest cannot read.
            execution_count: 0,
            id: manifest.id,
            name: manifest.name,
            version: manifest.version,
            description: manifest.metadata.description,
            author: manifest.metadata.author,
            homepage: manifest.metadata.homepage,
            support_url: manifest.metadata.support_url,
            license: manifest.metadata.license,
            provider_slug,
            plugin_type: manifest.plugin_type.as_str().to_owned(),
            api_version: manifest.api_version,
            capabilities,
            domains,
            max_concurrent_downloads: manifest.max_concurrent_downloads,
        }
    }
}

/// An installed package this build refuses to run.
#[derive(Serialize, ToSchema)]
pub struct IncompatiblePluginResponse {
    /// Plugin id, or the directory name when the manifest cannot say.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Stable code the UI translates: `plugin.manifest_outdated`,
    /// `plugin.capability_unknown` or `plugin.manifest_unreadable`.
    pub code: String,
}

impl From<rd_plugin_host::IncompatiblePlugin> for IncompatiblePluginResponse {
    fn from(plugin: rd_plugin_host::IncompatiblePlugin) -> Self {
        Self {
            id: plugin.id,
            name: plugin.name,
            version: plugin.version,
            code: plugin.code,
        }
    }
}

/// One recorded plugin invocation, as the diagnostics card shows it.
#[derive(Serialize, ToSchema)]
pub struct PluginExecutionResponse {
    /// Bare UUID a user can quote in a report; it identifies the entry and nothing else.
    pub correlation_id: String,
    pub plugin_version: String,
    pub plugin_type: String,
    /// Entry point that ran: `resolve`, `check`, `probe`, `run`, …
    pub operation: String,
    /// `ok`, `failed`, `crash`, `timeout`, `fuel`, `memory`, `host_error` or `denied`.
    pub outcome: String,
    /// Stable failure code, when there was one.
    pub error_class: Option<String>,
    /// Redacted message; never a credential, a token or a header value.
    pub message: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub duration_ms: i64,
}

impl From<rd_db::PluginExecution> for PluginExecutionResponse {
    fn from(entry: rd_db::PluginExecution) -> Self {
        Self {
            correlation_id: entry.correlation_id,
            plugin_version: entry.plugin_version,
            plugin_type: entry.plugin_type,
            operation: entry.operation,
            outcome: entry.outcome,
            error_class: entry.error_class,
            message: entry.message,
            started_at: entry.started_at,
            duration_ms: entry.duration_ms,
        }
    }
}

/// One plugin signing key the user confirmed on first use.
#[derive(Serialize, ToSchema)]
pub struct PluginTrustedKeyResponse {
    pub key_id: String,
    /// Hex SHA-256 of the public key, as shown when it was confirmed.
    pub fingerprint: String,
    pub plugin_name: Option<String>,
    pub confirmed_at: String,
}

impl From<rd_db::PluginTrustedKey> for PluginTrustedKeyResponse {
    fn from(key: rd_db::PluginTrustedKey) -> Self {
        Self {
            key_id: key.key_id,
            fingerprint: key.fingerprint,
            plugin_name: key.plugin_name,
            confirmed_at: key.confirmed_at,
        }
    }
}

/// One withdrawn plugin package version.
#[derive(Serialize, ToSchema)]
pub struct PluginDigestRevocationResponse {
    /// The package's content digest, 64 lowercase hex characters.
    pub digest: String,
    /// Which plugin it belonged to, when that was known when it was withdrawn.
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub version: Option<String>,
    pub reason: Option<String>,
    pub revoked_at: String,
}

impl From<rd_db::PluginDigestRevocation> for PluginDigestRevocationResponse {
    fn from(row: rd_db::PluginDigestRevocation) -> Self {
        Self {
            digest: row.digest,
            plugin_id: row.plugin_id,
            plugin_name: row.plugin_name,
            version: row.version,
            reason: row.reason,
            revoked_at: row.revoked_at,
        }
    }
}

/// Whether a provider resolves links for its own domains or other hosters' domains
/// (mirrors `rd_provider_registry::ProviderKind`).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKindResponse {
    Hoster,
    Multihoster,
}

impl From<rd_provider_registry::ProviderKind> for ProviderKindResponse {
    fn from(kind: rd_provider_registry::ProviderKind) -> Self {
        match kind {
            rd_provider_registry::ProviderKind::Hoster => Self::Hoster,
            rd_provider_registry::ProviderKind::Multihoster => Self::Multihoster,
        }
    }
}

/// The shape of the credential(s) a provider account stores (mirrors
/// `rd_provider_registry::CredentialKind`).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialsResponse {
    ApiKey,
    UsernamePassword,
    ApiKeyOrCookies,
    Cookies,
    /// One account, two ways to hold it; the settings form asks which one before it asks for
    /// the credential itself. The choices are in `credential_modes`.
    LoginOrApiKey,
    /// Signed in through a redirect; the settings form offers a sign-in rather than a field.
    ///
    /// Renamed for the same reason the manifest spelling is: the derived name would be
    /// `o_auth`, which is nobody's idea of what this is called.
    #[serde(rename = "oauth")]
    OAuth,
    /// Signed in with a code, or holding a pasted API key, chosen per account (RD-150-09); the
    /// choices are in `credential_modes`, and in the `oauth` one the form offers a sign-in
    /// rather than a field.
    #[serde(rename = "oauth_or_api_key")]
    OAuthOrApiKey,
    /// Takes no account; the accounts settings leave such a provider out of the list.
    #[serde(rename = "none")]
    NoneRequired,
}

impl From<rd_provider_registry::CredentialKind> for ProviderCredentialsResponse {
    fn from(kind: rd_provider_registry::CredentialKind) -> Self {
        match kind {
            rd_provider_registry::CredentialKind::ApiKey => Self::ApiKey,
            rd_provider_registry::CredentialKind::UsernamePassword => Self::UsernamePassword,
            rd_provider_registry::CredentialKind::ApiKeyOrCookies => Self::ApiKeyOrCookies,
            rd_provider_registry::CredentialKind::Cookies => Self::Cookies,
            rd_provider_registry::CredentialKind::LoginOrApiKey => Self::LoginOrApiKey,
            rd_provider_registry::CredentialKind::OAuth => Self::OAuth,
            rd_provider_registry::CredentialKind::OAuthOrApiKey => Self::OAuthOrApiKey,
            rd_provider_registry::CredentialKind::NoneRequired => Self::NoneRequired,
        }
    }
}

/// One entry of the provider registry, exposed for the accounts settings UI.
#[derive(Serialize, ToSchema)]
pub struct ProviderResponse {
    pub slug: String,
    pub display_name: String,
    pub kind: ProviderKindResponse,
    pub credentials: ProviderCredentialsResponse,
    pub username_required: bool,
    /// The credential modes this provider offers, in the order the form should present them.
    /// Empty for every provider with only one way to hold an account.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_modes: Vec<rd_provider_registry::CredentialMode>,
    /// Whether this provider is compiled in or contributed by an installed plugin.
    pub source: ProviderSourceResponse,
    /// Whether an installed plugin can sign this provider in without a key being typed
    /// (RD-090-13). A run-time fact, not a registry one: it depends on what is installed.
    #[serde(default)]
    pub device_flow: bool,
    /// The plugin behind this provider, and the version of it that is in use.
    ///
    /// Two versions of one plugin can be installed at once and the highest wins. That is well
    /// defined but was invisible: the dropdown said "DDownload" either way, so nobody could tell
    /// which one an account would actually be served by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    /// The host of the provider's `cookie_scope`, when its plugin declares one: the one site
    /// whose session the browser extension can hand over to an account (RD-120-45).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_scope_host: Option<String>,
}

/// Where a provider row came from (mirrors `rd_provider_registry::ProviderSource`).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSourceResponse {
    Builtin,
    Plugin,
}

impl From<rd_provider_registry::ProviderSource> for ProviderSourceResponse {
    fn from(source: rd_provider_registry::ProviderSource) -> Self {
        match source {
            rd_provider_registry::ProviderSource::Builtin => Self::Builtin,
            rd_provider_registry::ProviderSource::Plugin => Self::Plugin,
        }
    }
}

impl From<&rd_provider_registry::ProviderSpec> for ProviderResponse {
    fn from(spec: &rd_provider_registry::ProviderSpec) -> Self {
        Self {
            slug: spec.slug.clone(),
            display_name: spec.display_name.clone(),
            kind: spec.kind.into(),
            credentials: spec.credentials.into(),
            username_required: spec.username_required,
            credential_modes: spec.credential_modes(),
            source: spec.source.into(),
            // Filled in by the handler, which knows what is installed; the registry does not.
            device_flow: false,
            plugin_id: spec.plugin_id.clone(),
            plugin_version: spec.plugin_version.clone(),
            cookie_scope_host: spec
                .cookie_scope
                .as_deref()
                .and_then(|scope| url::Url::parse(scope).ok())
                .filter(|scope| scope.scheme() == "https")
                .and_then(|scope| scope.host_str().map(str::to_owned)),
        }
    }
}

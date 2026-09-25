//! Parameters for the configuration write tools: routing, storage and credentials.
//!
//! ## Why these mirror the REST bodies rather than reuse them
//!
//! The REST request bodies derive `utoipa::ToSchema`; an MCP tool needs `schemars::JsonSchema`,
//! and adding that derive would push a second schema framework through `rd-core` for every id
//! and enum they mention. The structs here are therefore a thin, string-typed mirror that is
//! converted into the REST body and validated by the REST handler — there is no second
//! validation and no second set of error codes.
//!
//! ## Create replaces, update merges
//!
//! REST `PUT` takes a full replacement, which is the right contract for a form that always has
//! every field in hand. An assistant does not: "make the Movies category green" would otherwise
//! mean resending its path, root and eight post-processing flags, and any one of them dropped
//! silently resets that setting. Update parameters are therefore all optional and merged onto
//! the stored row before the same handler sees them. `clear` names the fields to reset to their
//! inherited default, because "absent" already means "keep".

use rmcp::schemars;
use serde::Deserialize;

use crate::ApiError;

/// Field names an update may reset to null, checked against the ones it actually has.
pub(crate) fn clearing(
    clear: Option<&Vec<String>>,
    allowed: &[&str],
) -> Result<Vec<String>, ApiError> {
    let Some(clear) = clear else {
        return Ok(Vec::new());
    };
    for field in clear {
        if !allowed.contains(&field.as_str()) {
            return Err(ApiError::bad_request(
                "request.unknown_field",
                format!("{field} is not a field this tool can clear"),
            )
            .with_param("field", field.clone()));
        }
    }
    Ok(clear.clone())
}

/// Resolves one nullable field: explicitly listed in `clear` wins, then the new value, then
/// what is stored.
pub(crate) fn merged<T>(
    cleared: &[String],
    field: &str,
    new: Option<T>,
    current: Option<T>,
) -> Option<T> {
    if cleared.iter().any(|name| name == field) {
        return None;
    }
    new.or(current)
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PostprocessLevelParam {
    None,
    Repair,
    Unpack,
    Delete,
}

impl From<PostprocessLevelParam> for rd_core::PostprocessLevel {
    fn from(value: PostprocessLevelParam) -> Self {
        match value {
            PostprocessLevelParam::None => Self::None,
            PostprocessLevelParam::Repair => Self::Repair,
            PostprocessLevelParam::Unpack => Self::Unpack,
            PostprocessLevelParam::Delete => Self::Delete,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IngressSourceParam {
    Manual,
    Clipboard,
    ClickAndLoad,
    Api,
    Nzb,
    HotFolder,
    BrowserExtension,
    BrowserDownload,
    Subscription,
}

impl From<IngressSourceParam> for rd_core::IngressSource {
    fn from(value: IngressSourceParam) -> Self {
        match value {
            IngressSourceParam::Manual => Self::Manual,
            IngressSourceParam::Clipboard => Self::Clipboard,
            IngressSourceParam::ClickAndLoad => Self::ClickAndLoad,
            IngressSourceParam::Api => Self::Api,
            IngressSourceParam::Nzb => Self::Nzb,
            IngressSourceParam::HotFolder => Self::HotFolder,
            IngressSourceParam::BrowserExtension => Self::BrowserExtension,
            IngressSourceParam::BrowserDownload => Self::BrowserDownload,
            IngressSourceParam::Subscription => Self::Subscription,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportModeParam {
    /// Imported links wait in the LinkGrabber for review.
    Review,
    /// Imported links go straight into the download queue.
    Enqueue,
}

impl From<ImportModeParam> for rd_core::ImportMode {
    fn from(value: ImportModeParam) -> Self {
        match value {
            ImportModeParam::Review => Self::Review,
            ImportModeParam::Enqueue => Self::Enqueue,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProxyKindParam {
    Http,
    Https,
    Socks5,
}

impl From<ProxyKindParam> for rd_core::ProxyKind {
    fn from(value: ProxyKindParam) -> Self {
        match value {
            ProxyKindParam::Http => Self::Http,
            ProxyKindParam::Https => Self::Https,
            ProxyKindParam::Socks5 => Self::Socks5,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CredentialModeParam {
    /// The stored secret is an account password.
    Login,
    /// The stored secret is a ready-made API key.
    ApiKey,
}

impl From<CredentialModeParam> for rd_provider_registry::CredentialMode {
    fn from(value: CredentialModeParam) -> Self {
        match value {
            CredentialModeParam::Login => Self::Login,
            CredentialModeParam::ApiKey => Self::ApiKey,
        }
    }
}

/// Just an id, for the tools that only name a row.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct IdParams {
    pub id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateCategoryParams {
    pub name: String,
    /// `#RRGGBB`.
    pub color: String,
    pub storage_root_id: String,
    /// Folder below the storage root; relative, no `..`.
    pub relative_path: String,
    /// Makes this the category new packages fall into.
    pub is_default: Option<bool>,
    /// Post-processing depth; omitted inherits the global level.
    pub postprocess_level: Option<PostprocessLevelParam>,
    /// User script run after post-processing, by file name.
    pub script: Option<String>,
    /// Extensions removed after unpacking; omitted inherits the global list.
    pub cleanup_extensions: Option<Vec<String>>,
    pub recursive_unpack: Option<bool>,
    pub sfv_verify: Option<bool>,
    /// Whether a failed verification blocks the unpack and everything after it.
    pub safe_postproc: Option<bool>,
    pub delete_par2: Option<bool>,
    pub upload_enabled: Option<bool>,
    /// rclone target in `remote:path` form.
    pub upload_remote: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateCategoryParams {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub storage_root_id: Option<String>,
    pub relative_path: Option<String>,
    pub is_default: Option<bool>,
    pub postprocess_level: Option<PostprocessLevelParam>,
    pub script: Option<String>,
    pub cleanup_extensions: Option<Vec<String>>,
    pub recursive_unpack: Option<bool>,
    pub sfv_verify: Option<bool>,
    /// Whether a failed verification blocks the unpack and everything after it.
    pub safe_postproc: Option<bool>,
    pub delete_par2: Option<bool>,
    pub upload_enabled: Option<bool>,
    pub upload_remote: Option<String>,
    /// Fields to reset to the global default: postprocess_level, script, cleanup_extensions,
    /// recursive_unpack, sfv_verify, safe_postproc, delete_par2, upload_enabled, upload_remote.
    pub clear: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateCategoryRuleParams {
    pub name: String,
    /// Lower numbers are tried first.
    pub priority: i32,
    /// Category the rule routes matching links into.
    pub category_id: String,
    pub enabled: Option<bool>,
    /// Only links that arrived this way.
    pub source: Option<IngressSourceParam>,
    /// Lowercase host, no scheme, port or path.
    pub domain: Option<String>,
    pub protocol: Option<String>,
    /// File extension, with or without the leading dot.
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    /// Regular expression matched against the file name.
    pub name_regex: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateCategoryRuleParams {
    pub id: String,
    pub name: Option<String>,
    pub priority: Option<i32>,
    pub category_id: Option<String>,
    pub enabled: Option<bool>,
    pub source: Option<IngressSourceParam>,
    pub domain: Option<String>,
    pub protocol: Option<String>,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub name_regex: Option<String>,
    /// Filters to drop: source, domain, protocol, extension, mime_type, name_regex.
    pub clear: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateStorageRootParams {
    pub name: String,
    /// Absolute directory; it is created and probed for writability.
    pub path: String,
    pub is_default: Option<bool>,
    /// Free space kept on this root, in bytes; omitted inherits the global reserve.
    pub minimum_free_bytes: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateStorageRootParams {
    pub id: String,
    pub name: Option<String>,
    pub path: Option<String>,
    pub is_default: Option<bool>,
    pub minimum_free_bytes: Option<u64>,
    /// Fields to reset to the global default: minimum_free_bytes.
    pub clear: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateHotfolderParams {
    pub name: String,
    /// Absolute directory watched for `.nzb`, `.dlc` and link lists.
    pub path: String,
    /// Whether subdirectories are watched too.
    pub recursive: Option<bool>,
    pub import_mode: ImportModeParam,
    /// Subfolder for files that imported cleanly, relative to `path`.
    pub processed_path: String,
    /// Subfolder for files that failed, relative to `path`.
    pub failed_path: String,
    pub category_id: Option<String>,
    pub enabled: Option<bool>,
    /// Capture agent that should watch the folder; omitted lets the service watch it.
    pub capture_agent_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateHotfolderParams {
    pub id: String,
    pub name: Option<String>,
    pub path: Option<String>,
    pub recursive: Option<bool>,
    pub import_mode: Option<ImportModeParam>,
    pub processed_path: Option<String>,
    pub failed_path: Option<String>,
    pub category_id: Option<String>,
    pub enabled: Option<bool>,
    pub capture_agent_id: Option<String>,
    /// Fields to drop: category_id, capture_agent_id (the service watches the folder again).
    pub clear: Option<Vec<String>>,
}

/// Account metadata only. The credential is deliberately absent — see `tools_credentials`.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateAccountParams {
    /// Provider slug from `list_configuration(section = "providers")`.
    pub provider: String,
    /// Display label.
    pub label: String,
    pub username: Option<String>,
    /// Which credential the vault entry holds, for providers that offer a choice.
    pub credential_mode: Option<CredentialModeParam>,
    pub proxy_profile_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateAccountParams {
    pub id: String,
    pub provider: Option<String>,
    pub label: Option<String>,
    pub username: Option<String>,
    pub credential_mode: Option<CredentialModeParam>,
    pub proxy_profile_id: Option<String>,
    pub enabled: Option<bool>,
    /// Fields to drop: username, credential_mode, proxy_profile_id.
    pub clear: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateProxyProfileParams {
    pub name: String,
    pub kind: ProxyKindParam,
    /// Proxy URL without credentials, e.g. `socks5://10.0.0.2:1080`.
    pub endpoint: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateProxyProfileParams {
    pub id: String,
    pub name: Option<String>,
    pub kind: Option<ProxyKindParam>,
    pub endpoint: Option<String>,
}

/// Name and address of a proxy. A proxy that needs a login is created in the web UI, because
/// its password is a credential and no tool here takes one.

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateUsenetServerParams {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Whether the connection uses TLS.
    pub tls: Option<bool>,
    pub username: Option<String>,
    /// Lower numbers are tried first; a backup server gets a higher number.
    pub priority: Option<i32>,
    pub max_connections: Option<u16>,
    pub proxy_profile_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateUsenetServerParams {
    pub id: String,
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub tls: Option<bool>,
    pub username: Option<String>,
    pub priority: Option<i32>,
    pub max_connections: Option<u16>,
    pub proxy_profile_id: Option<String>,
    pub enabled: Option<bool>,
    /// Fields to drop: username, proxy_profile_id.
    pub clear: Option<Vec<String>>,
}

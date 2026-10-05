//! Authentication profiles, captured cookies and stored logins for remote servers.

use super::*;

/// Reusable per-domain session or authentication profile. Credential fields are write-only
/// and never appear in a response.
#[derive(Deserialize, ToSchema)]
pub struct CreateAuthProfileRequest {
    pub name: String,
    /// Bare host or full URL; the path selects which profile matches a URL.
    pub scope: String,
    pub include_subdomains: bool,
    pub method: rd_core::AuthMethod,
    /// Username for `basic`.
    pub username: Option<String>,
    /// Netscape `cookies.txt` content or a `Cookie` header for `cookies`; the password for
    /// `basic`; the token for `bearer`.
    #[schema(write_only)]
    pub secret: Option<String>,
    /// Private key and certificate chain as one PEM bundle.
    #[schema(write_only)]
    pub certificate_pem: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub enabled: bool,
}

/// Editable profile fields. Empty credential fields preserve their stored values.
#[derive(Deserialize, ToSchema)]
pub struct UpdateAuthProfileRequest {
    pub name: String,
    pub scope: String,
    pub include_subdomains: bool,
    pub method: rd_core::AuthMethod,
    pub username: Option<String>,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub certificate_pem: Option<String>,
    pub clear_certificate: bool,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub enabled: bool,
}

/// Cookies handed over by a capture client for one approved domain.
#[derive(Deserialize, ToSchema)]
pub struct CaptureCookiesRequest {
    pub name: Option<String>,
    pub scope: String,
    pub include_subdomains: bool,
    #[schema(write_only)]
    pub cookies: String,
}

/// Which profile a single download uses.
#[derive(Deserialize, ToSchema)]
pub struct SetDownloadAuthProfileRequest {
    pub auth_profile: rd_core::AuthProfileSelection,
}

/// Redaction-safe result of a live auth profile check.
#[derive(Serialize, ToSchema)]
pub struct AuthProfileTestResponse {
    pub reachable: bool,
    pub authenticated: bool,
    pub status: Option<u16>,
    pub url: String,
}

/// A new stored login for an FTP, FTPS or SFTP server.
///
/// Credential fields are write-only: they are handed to the secret store on arrival and
/// no endpoint ever returns them or their `vault://` reference.
#[derive(Deserialize, ToSchema)]
pub struct CreateRemoteCredentialRequest {
    pub name: String,
    pub protocol: rd_core::RemoteProtocol,
    /// Bare host name or IP address; a URL is accepted and reduced to its host.
    pub host: String,
    /// `None` uses the protocol's default port.
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth_mode: rd_core::RemoteAuthMode,
    /// FTP data connections use PASV/EPSV; `false` asks for active mode instead.
    #[serde(default = "default_true")]
    pub passive: bool,
    /// Password for `password`, unused for the other modes.
    #[schema(write_only)]
    pub secret: Option<String>,
    /// OpenSSH or PEM private key for `private_key`.
    #[schema(write_only)]
    pub private_key: Option<String>,
    /// Passphrase protecting `private_key`, when it has one.
    #[schema(write_only)]
    pub passphrase: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Editable login fields. Empty credential fields preserve their stored values.
#[derive(Deserialize, ToSchema)]
pub struct UpdateRemoteCredentialRequest {
    pub name: String,
    pub protocol: rd_core::RemoteProtocol,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth_mode: rd_core::RemoteAuthMode,
    #[serde(default = "default_true")]
    pub passive: bool,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub private_key: Option<String>,
    #[schema(write_only)]
    pub passphrase: Option<String>,
    /// Drops the stored private key instead of keeping it.
    #[serde(default)]
    pub clear_private_key: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Redaction-safe result of a live remote login check.
#[derive(Serialize, ToSchema)]
pub struct RemoteCredentialTestResponse {
    pub reachable: bool,
    pub authenticated: bool,
    /// Stable failure code when the check did not succeed.
    pub code: Option<String>,
    /// Parameters belonging to `code`, including an SSH fingerprint awaiting confirmation.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: rd_core::MessageParams,
}

/// Confirms one SSH host key as trusted.
///
/// The fingerprint is required rather than implied: confirming "whatever the server offers
/// next" would make the trust store meaningless, so the caller has to name the key it saw.
#[derive(Deserialize, ToSchema)]
pub struct TrustSshHostKeyRequest {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:<base64>`, exactly as reported by the failure that blocked the transfer.
    pub fingerprint: String,
}

/// The file selection of one remote directory candidate.
#[derive(Deserialize, ToSchema)]
pub struct RemoteListingPlanRequest {
    /// Paths that are excluded; excluding a directory excludes everything below it.
    #[serde(default)]
    pub excluded: Vec<String>,
}

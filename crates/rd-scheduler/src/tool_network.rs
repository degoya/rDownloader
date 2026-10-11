//! The proxy and the trust a download tool is started with (RD-1240-08).
//!
//! yt-dlp, gallery-dl and streamlink open their own sockets, so the client the queue builds for
//! a file never carries their bytes. Until 1.24 they inherited only the service's own
//! `HTTP(S)_PROXY`, and a person with a proxy profile fetched media, galleries and streams past
//! it. A runner now asks [`ToolNetworkSource::for_file`] for the file's network - the proxy
//! resolved with the precedence every transfer uses (the job's profile, the account's, the
//! global one) and the custom CA - and hands both to its tool.
//!
//! Credentials never reach an argument list, which every process on the machine can read. A
//! profile without credentials goes to the tool as its own option and through the environment;
//! one with credentials only through the child's environment, which only its own user can read.
//! A proxy that cannot be resolved fails the run; it never turns into a direct connection.

use std::{fmt::Write as _, path::Path};

use anyhow::{Context, Result};
use rd_core::{AccountId, DownloadFile, Failure, FailureKind, ProxyKind, ProxyProfileId};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// The proxy variables the tools read, in both spellings: Python prefers the lower-case one,
/// so a service started with `https_proxy` set would otherwise win over the profile.
const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// Exceptions a profile does not have. The HTTP engine ignores the service's own as well.
const NO_PROXY_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];

/// The CA file OpenSSL (`SSL_CERT_FILE`) and requests (`REQUESTS_CA_BUNDLE`) read.
const TRUST_VARIABLES: [&str; 2] = ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE"];

/// What a tool prints when it cannot speak the proxy's protocol: yt-dlp without the request
/// handler a scheme needs, requests without PySocks, urllib3 for a scheme it does not know.
const UNSUPPORTED_PROXY_MARKERS: [&str; 3] = [
    "unsupported proxy type",
    "missing dependencies for socks support",
    "proxy url had unsupported scheme",
];

/// The proxy one tool run goes through.
#[derive(Clone)]
pub struct ToolProxy {
    kind: ProxyKind,
    /// The profile's endpoint, without credentials: safe for an argument list and a log line.
    endpoint: Url,
    /// The endpoint with the profile's credentials, when it has any.
    credentialed: Option<SecretString>,
    /// The same credentials undecoded, for a client of the service's own (RD-1240-22).
    basic_auth: Option<(String, SecretString)>,
}

impl std::fmt::Debug for ToolProxy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolProxy")
            .field("kind", &self.kind)
            .field("endpoint", &self.endpoint.as_str())
            .field("has_credentials", &self.credentialed.is_some())
            .finish()
    }
}

impl ToolProxy {
    /// A proxy at `endpoint`, signed in as `username` with `password` when the profile has them.
    pub fn new(
        kind: ProxyKind,
        mut endpoint: Url,
        username: Option<&str>,
        password: Option<&SecretString>,
    ) -> Result<Self> {
        // A stored profile never has userinfo in its endpoint; the API refuses one
        // (`proxy.credentials_in_url`). Stripped anyway, so nothing below can leak it.
        let _ = endpoint.set_password(None);
        let _ = endpoint.set_username("");
        let credentialed = match (username, password) {
            (None, None) => None,
            (username, password) => {
                let mut url = endpoint.clone();
                url.set_username(&userinfo_encode(username.unwrap_or_default()))
                    .ok()
                    .context("proxy endpoint cannot carry a user name")?;
                if let Some(password) = password {
                    url.set_password(Some(&userinfo_encode(password.expose_secret())))
                        .ok()
                        .context("proxy endpoint cannot carry a password")?;
                }
                Some(SecretString::from(url.to_string()))
            }
        };
        let basic_auth = credentialed.is_some().then(|| {
            (
                username.unwrap_or_default().to_owned(),
                password
                    .cloned()
                    .unwrap_or_else(|| SecretString::from(String::new())),
            )
        });
        Ok(Self {
            kind,
            endpoint,
            credentialed,
            basic_auth,
        })
    }

    /// The proxy for a `reqwest` client of the service's own, built as `rd-http` builds a
    /// download's (RD-1240-22): the endpoint, and the credentials as basic authentication.
    pub fn http_proxy(&self) -> Result<reqwest::Proxy> {
        let mut proxy = reqwest::Proxy::all(self.endpoint.as_str()).context("create proxy")?;
        if let Some((username, password)) = &self.basic_auth {
            proxy = proxy.basic_auth(username, password.expose_secret());
        }
        Ok(proxy)
    }

    #[must_use]
    pub const fn kind(&self) -> ProxyKind {
        self.kind
    }

    /// The endpoint without credentials.
    #[must_use]
    pub const fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    #[must_use]
    pub const fn has_credentials(&self) -> bool {
        self.credentialed.is_some()
    }

    /// The value for the tool's own proxy option; `None` when the profile has credentials,
    /// which go through the environment only.
    #[must_use]
    pub fn argument(&self) -> Option<&str> {
        self.credentialed.is_none().then(|| self.endpoint.as_str())
    }

    /// The value the proxy variables carry, credentials included.
    fn environment_value(&self) -> &str {
        self.credentialed
            .as_ref()
            .map_or(self.endpoint.as_str(), |url| url.expose_secret())
    }
}

/// Percent-encodes everything but the unreserved characters, so a `+`, `:` or `@` in a user
/// name or password reaches the tool as itself: Python decodes the userinfo with `unquote`,
/// and yt-dlp's SOCKS path even with `unquote_plus`.
fn userinfo_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// What one tool run is started with: the file's proxy and the custom CA.
#[derive(Debug, Default)]
pub struct ToolNetwork {
    proxy: Option<ToolProxy>,
    /// The platform roots plus the custom CA, removed when the run's network is dropped.
    trust_bundle: Option<tempfile::TempPath>,
}

impl ToolNetwork {
    /// No proxy and no custom CA: the tool decides alone, as before RD-1240-08.
    #[must_use]
    pub fn direct() -> Self {
        Self::default()
    }

    /// A network with `proxy` and no custom CA.
    #[must_use]
    pub fn with_proxy(proxy: ToolProxy) -> Self {
        Self {
            proxy: Some(proxy),
            trust_bundle: None,
        }
    }

    #[must_use]
    pub const fn proxy(&self) -> Option<&ToolProxy> {
        self.proxy.as_ref()
    }

    /// The value for the tool's own proxy option, see [`ToolProxy::argument`].
    #[must_use]
    pub fn proxy_argument(&self) -> Option<&str> {
        self.proxy.as_ref().and_then(ToolProxy::argument)
    }

    /// Whether the proxy has to reach the tool through its environment alone.
    #[must_use]
    pub fn proxy_in_environment_only(&self) -> bool {
        self.proxy.as_ref().is_some_and(ToolProxy::has_credentials)
    }

    /// The CA file the tool is to trust, when a custom CA is configured.
    #[must_use]
    pub fn trust_bundle(&self) -> Option<&Path> {
        self.trust_bundle.as_deref()
    }

    /// Sets the child's proxy and CA variables; nothing at all for [`Self::direct`].
    ///
    /// Set on the command before it is spawned, so `rd_files::restrict_environment` keeps them
    /// on top of what it passes through from the service.
    pub fn apply(&self, command: &mut tokio::process::Command) {
        if let Some(proxy) = &self.proxy {
            for name in PROXY_VARIABLES {
                command.env(name, proxy.environment_value());
            }
            for name in NO_PROXY_VARIABLES {
                command.env_remove(name);
            }
        }
        if let Some(bundle) = self.trust_bundle() {
            for name in TRUST_VARIABLES {
                command.env(name, bundle);
            }
        }
    }

    /// The failure for a tool that said it cannot use this proxy, `None` when its output says
    /// nothing of the kind or no proxy was set. The tool stopped rather than going direct; the
    /// failure says why, instead of a generic tool error.
    #[must_use]
    pub fn unsupported_proxy(&self, tool: &str, stderr: &str) -> Option<Failure> {
        let proxy = self.proxy.as_ref()?;
        let lower = stderr.to_ascii_lowercase();
        UNSUPPORTED_PROXY_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
            .then(|| unsupported_by_tool(tool, proxy.kind))
    }
}

/// `proxy.unsupported_by_tool`: the tool cannot use this kind of proxy, and was not let go
/// without it.
#[must_use]
pub fn unsupported_by_tool(tool: &str, kind: ProxyKind) -> Failure {
    let kind = match kind {
        ProxyKind::Http => "HTTP",
        ProxyKind::Https => "HTTPS",
        ProxyKind::Socks5 => "SOCKS5",
    };
    Failure::coded(
        FailureKind::Unsupported,
        "proxy.unsupported_by_tool",
        format!("{tool} cannot use a {kind} proxy; the download was not started without it"),
    )
    .with_param("tool", tool)
    .with_param("kind", kind)
}

/// `proxy.auth_failed`: the proxy answered the tool's request with 407, it refused the
/// profile's user name or password; `None` when `output` says nothing of the kind
/// (RD-1240-28, RD-1240-29).
///
/// Read before a tool's own sign-in and its retryable fallback: "Proxy Authentication
/// Required" says "authentication" too, and a retry in two minutes only meets the same
/// refusal. Matched are the reason phrase (urllib3's and Python's "Tunnel connection failed:
/// 407 Proxy Authentication Required", an `HTTP Error 407`) and a bare 407 next to "proxy" or
/// "tunnel" (curl's "CONNECT tunnel failed, response 407").
#[must_use]
pub fn proxy_auth_failed(output: &str) -> Option<Failure> {
    let lower = output.to_ascii_lowercase();
    let refused = lower.contains("proxy authentication required")
        || (says_407(&lower) && (lower.contains("proxy") || lower.contains("tunnel")));
    refused.then(|| {
        Failure::coded(
            FailureKind::Permanent,
            "proxy.auth_failed",
            "The proxy refused the sign-in (407); check the user name and password in the proxy \
             profile",
        )
    })
}

/// Whether `text` carries 407 as a number of its own, not inside an id or a longer number.
fn says_407(text: &str) -> bool {
    let is_word = |character: Option<char>| character.is_some_and(char::is_alphanumeric);
    text.match_indices("407").any(|(start, _)| {
        !is_word(text[..start].chars().next_back()) && !is_word(text[start + 3..].chars().next())
    })
}

/// `proxy.unavailable`: the file's proxy could not be resolved, so its tool was not started.
fn proxy_unavailable(error: &anyhow::Error) -> Failure {
    let reason = rd_core::redact_text(&format!("{error:#}"));
    Failure::coded(
        FailureKind::Permanent,
        "proxy.unavailable",
        format!(
            "The proxy for this download cannot be used, so the download was not started \
             without it: {reason}"
        ),
    )
    .with_param("reason", reason)
}

/// `proxy.check_unavailable`: the proxy for a request that belongs to no download could not be
/// resolved, so the request was not made (RD-1240-22).
fn check_proxy_unavailable(error: &anyhow::Error) -> Failure {
    let reason = rd_core::redact_text(&format!("{error:#}"));
    Failure::coded(
        FailureKind::Permanent,
        "proxy.check_unavailable",
        format!("The proxy cannot be used, so the check was not made without it: {reason}"),
    )
    .with_param("reason", reason)
}

/// Resolves the network a tool run is started with.
///
/// Built once at start-up from the handles the scheduler is built from, because the runners
/// are registered with the scheduler before it exists.
#[derive(Clone)]
pub struct ToolNetworkSource {
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    defaults: rd_http::SharedNetworkDefaults,
}

impl ToolNetworkSource {
    #[must_use]
    pub fn new(
        database: rd_db::Database,
        secrets: rd_secrets::SecretStore,
        defaults: rd_http::SharedNetworkDefaults,
    ) -> Self {
        Self {
            database,
            secrets,
            defaults,
        }
    }

    /// The network for `file`, or the failure that ends its run before the tool starts.
    pub async fn for_file(&self, file: &DownloadFile) -> Result<ToolNetwork, Failure> {
        self.resolve(file.account_id, file.proxy_profile_id, &file.source)
            .await
            .map_err(|error| proxy_unavailable(&error))
    }

    /// The network for a request that belongs to no download (RD-1240-22): the LinkGrabber's
    /// media probe and the channel monitor. A candidate, a subscription and a channel carry
    /// neither a profile nor an account, so it is the global profile, as for the HTTP online
    /// check; without one, no proxy.
    pub async fn for_request(&self, url: &Url) -> Result<ToolNetwork, Failure> {
        self.resolve(None, None, url)
            .await
            .map_err(|error| check_proxy_unavailable(&error))
    }

    /// The proxy with the precedence of every transfer (`network_client_config`): the job's
    /// profile, the account's, the global one; plus the custom CA as a file.
    pub async fn resolve(
        &self,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        source: &Url,
    ) -> Result<ToolNetwork> {
        let defaults = self.defaults.read().await.clone();
        let config = self
            .database
            .network_client_config(
                account_id,
                proxy_profile_id,
                defaults.global_proxy_profile_id,
                rd_core::AuthProfileSelection::None,
                source,
            )
            .await?;
        let proxy = match config.proxy {
            Some(profile) => {
                let password = match profile.secret_ref.as_deref() {
                    Some(reference) => Some(
                        self.secrets
                            .get(reference)
                            .await
                            .context("the proxy password cannot be read")?,
                    ),
                    None => None,
                };
                Some(ToolProxy::new(
                    profile.kind,
                    profile.endpoint,
                    profile.username.as_deref(),
                    password.as_ref(),
                )?)
            }
            None => None,
        };
        Ok(ToolNetwork {
            proxy,
            trust_bundle: trust_bundle(defaults.custom_ca_pem).await,
        })
    }
}

/// Writes the platform roots plus the custom CA to a file of its own; `None` without a custom
/// CA. A bundle that cannot be written leaves the tool its own roots, as before RD-1240-08: it
/// costs a site behind the custom CA, never a connection past the proxy.
async fn trust_bundle(custom_ca_pem: Vec<Vec<u8>>) -> Option<tempfile::TempPath> {
    if custom_ca_pem.is_empty() {
        return None;
    }
    let written = tokio::task::spawn_blocking(move || -> Result<Option<tempfile::TempPath>> {
        let Some(bundle) = rd_http::tool_trust_bundle(&custom_ca_pem)? else {
            return Ok(None);
        };
        let mut file = tempfile::Builder::new()
            .prefix("rdownloader-ca-")
            .suffix(".pem")
            .tempfile()
            .context("create the CA bundle file")?;
        std::io::Write::write_all(&mut file, bundle.as_bytes()).context("write the CA bundle")?;
        Ok(Some(file.into_temp_path()))
    })
    .await;
    match written {
        Ok(Ok(bundle)) => bundle,
        Ok(Err(error)) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                "the CA bundle for a download tool could not be written"
            );
            None
        }
        Err(error) => {
            tracing::warn!(%error, "the CA bundle for a download tool could not be written");
            None
        }
    }
}

#[cfg(test)]
#[path = "tool_network_tests.rs"]
mod tests;

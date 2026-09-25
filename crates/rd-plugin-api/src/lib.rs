//! Native resolver trait and WebAssembly component contract types.

use std::time::Duration;

use async_trait::async_trait;
use rd_core::{
    AccountId, ByteCount, ChecksumAlgorithm, Failure, FailureKind, LinkCheckResult, MessageParams,
    PluginId, ProxyProfileId,
};
use serde::{Deserialize, Serialize};
use url::Url;

/// Waiting time a host reserves for one captcha when nothing better is known.
pub const DEFAULT_CAPTCHA_ALLOWANCE: Duration = Duration::from_secs(180);

/// Header or query field whose secret placeholders are expanded by the host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostRequestValue {
    pub name: String,
    pub value_template: String,
}

/// One stage of a key derivation the host performs over a credential (RD-120-20).
///
/// The guest names the stages; the host runs them. Each stage's output is the next one's
/// input, the first stage's input is the credential itself, and only the last stage's output
/// is handed back — so the credential and every intermediate value stay here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DerivationStep {
    /// PBKDF2-HMAC-SHA512 over the running value.
    Pbkdf2HmacSha512 {
        /// The provider's salt, as the provider sent it.
        salt: Vec<u8>,
        /// Iterations. The host refuses anything below its floor.
        rounds: u32,
        /// How many bytes of output the next stage works on.
        length: u32,
    },
    /// AES-128-ECB, decrypting these bytes under the first sixteen bytes of the running
    /// value.
    Aes128EcbDecrypt(Vec<u8>),
    /// Narrows the running value to a window of itself.
    Take { offset: u32, length: u32 },
}

/// Network request description; plugins never receive a socket or clear-text secret.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostHttpRequest {
    pub method: String,
    pub url: Url,
    pub query: Vec<HostRequestValue>,
    pub headers: Vec<HostRequestValue>,
    pub body: Vec<u8>,
    /// Which allowlist decides where this request may go.
    #[serde(default)]
    pub authority: RequestAuthority,
    /// Whether this invocation may use the methods that change something at the far end.
    ///
    /// Only a storage destination may: uploading is what it is for, and `PUT`, `MKCOL`,
    /// `PROPFIND` and `DELETE` are how WebDAV spells it. Every other plugin type reads, so
    /// the set stays `GET`, `POST` and `HEAD` — a resolver that could `DELETE` would be a
    /// permission nobody asked for and nothing checks.
    #[serde(default)]
    pub write_methods: bool,
    /// The one vault reference this invocation may expand into `{{secret}}`, if any.
    ///
    /// Set by the host from what it granted the invocation, never by the guest — which is
    /// why it lives here rather than in the request the plugin describes. A plugin writes
    /// `{{secret}}` without a reference precisely because it has no business naming one.
    #[serde(default)]
    pub granted_secret: Option<String>,
}

/// Which allowlist an outgoing request is measured against.
///
/// Both cases check the plugin's own manifest — that never moves. What differs is whether a
/// second, wider net applies underneath it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestAuthority {
    /// A resolver, which serves a provider: the provider registry's request domains apply as
    /// well, so a resolver cannot reach an address no provider in this build declares.
    #[default]
    Provider,
    /// An extension type, which serves no provider: the registry's union says nothing about
    /// where an ntfy topic or a WebDAV server lives, so the manifest is the whole answer.
    Manifest,
}

/// Bounded response returned by the trusted host after redirect validation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostHttpResponse {
    pub status: u16,
    pub final_url: Url,
    pub headers: Vec<ResolvedHeader>,
    pub body: Vec<u8>,
}

impl HostHttpResponse {
    /// Looks up a response header case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }
}

/// Metadata used to order and constrain a resolver implementation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolverMetadata {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: String,
    /// Provider slug this resolver serves; the only key resolver dispatch matches on.
    pub provider_slug: String,
    pub domains: Vec<String>,
    pub max_concurrent_downloads: u32,
    pub requires_account: bool,
}

/// The fields of a bundled `manifest.toml` that describe the resolver behind it.
///
/// Deliberately a subset: the host owns the full manifest (signing key, capabilities,
/// limits), while a plugin's own native build only needs to know what it *is*. Serde
/// ignores the rest, and a renamed field fails to deserialise loudly instead of silently
/// reading a default.
#[derive(Deserialize)]
struct BundledManifest {
    id: PluginId,
    name: String,
    version: String,
    #[serde(default)]
    match_domains: Vec<String>,
    max_concurrent_downloads: u32,
    #[serde(default = "requires_account_default")]
    requires_account: bool,
    capabilities: BundledCapabilities,
    provider: BundledProvider,
}

#[derive(Deserialize)]
struct BundledCapabilities {
    net_http: BundledNetHttp,
}

#[derive(Deserialize)]
struct BundledNetHttp {
    domains: Vec<String>,
}

#[derive(Deserialize)]
struct BundledProvider {
    slug: String,
}

fn requires_account_default() -> bool {
    true
}

/// Reads a plugin's own bundled `manifest.toml` into its resolver metadata.
///
/// The manifest is the single authority for what a resolver is. Both builds of a plugin read
/// the same file — the component through the host that installed its package, the native
/// fallback through `include_str!` — so identity, domain list, concurrency and version can no
/// longer drift apart between the two. In particular the reported version becomes the plugin's
/// own, not the workspace's, so a job pinned to a resolver still finds it after a core release
/// that did not touch the plugin.
///
/// Reads the same domain list the component path reports: `match_domains` when the manifest
/// names one, otherwise the network allowlist.
///
/// # Panics
///
/// The manifest is embedded at compile time and validated by packaging and the bundled
/// manifest tests, so a parse failure here is a build defect, not a runtime condition.
#[must_use]
pub fn metadata_from_manifest(source: &str) -> ResolverMetadata {
    let manifest: BundledManifest =
        toml::from_str(source).expect("bundled plugin manifest is well-formed");
    let domains = if manifest.match_domains.is_empty() {
        manifest.capabilities.net_http.domains
    } else {
        manifest.match_domains
    };
    ResolverMetadata {
        plugin_id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        provider_slug: manifest.provider.slug,
        domains,
        max_concurrent_downloads: manifest.max_concurrent_downloads,
        requires_account: manifest.requires_account,
    }
}

/// Network identity that must be preserved from resolving through transfer.
///
/// Anonymous free downloads rely on this: an account-less identity resolves to the same
/// pooled client — and therefore the same cookie jar — that the transfer later uses, so a
/// session established while resolving still applies when the bytes are fetched. The jar is
/// shared by all anonymous work, which is why a hoster's free flows must not run in
/// parallel (`max_concurrent_downloads` in the plugin manifest).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientIdentity {
    pub account_id: Option<AccountId>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub tls_revision: u64,
}

/// A captcha the resolver cannot solve itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CaptchaChallenge {
    RecaptchaV2(WidgetChallenge),
    HCaptcha(WidgetChallenge),
    Turnstile(WidgetChallenge),
    Image(ImageChallenge),
    /// A picture answered by clicking one spot in it (RD-110-15).
    ClickPoint(ImageChallenge),
    /// A CutCaptcha widget, which only a solver service answers (RD-110-15).
    Cutcaptcha(CutcaptchaChallenge),
}

impl CaptchaChallenge {
    /// The hoster page a widget challenge is rendered on, or `None` for a picture.
    ///
    /// A widget token is only valid for the origin that produced it, so the page is what
    /// every consumer of a widget challenge needs: the solver service to reproduce it, the
    /// host to check it against the plugin's declared domains, and the browser extension to
    /// know which page a person is about to be shown.
    #[must_use]
    pub fn page_url(&self) -> Option<&str> {
        match self {
            Self::RecaptchaV2(widget) | Self::HCaptcha(widget) | Self::Turnstile(widget) => {
                Some(widget.page_url.as_str())
            }
            Self::Cutcaptcha(widget) => Some(widget.page_url.as_str()),
            Self::Image(_) | Self::ClickPoint(_) => None,
        }
    }

    /// Whether the answer is a coordinate rather than a token or typed text.
    #[must_use]
    pub const fn answers_with_point(&self) -> bool {
        matches!(self, Self::ClickPoint(_))
    }
}

/// Widget captcha, solvable from its site key and the page it is embedded in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WidgetChallenge {
    pub site_key: String,
    pub page_url: String,
    pub invisible: bool,
}

/// Classic image captcha as served by the hoster.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImageChallenge {
    pub mime: String,
    pub data: Vec<u8>,
    pub prompt: Option<String>,
}

/// CutCaptcha widget as its solver task needs it: the widget's own identifier and the page's
/// misery key, both read from the hoster page, plus the page itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CutcaptchaChallenge {
    pub site_key: String,
    pub misery_key: String,
    pub page_url: String,
}

/// Where a person clicked in a click-point captcha, in pixels of the image as served.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClickPoint {
    pub x: u32,
    pub y: u32,
}

/// The answer to a challenge, in the shape the challenge has: a widget token or the typed
/// text of an image captcha, or the spot clicked in a click-point captcha.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CaptchaAnswer {
    Token(String),
    Point(ClickPoint),
}

impl CaptchaAnswer {
    /// The token, or `None` for a point.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Token(token) => Some(token),
            Self::Point(_) => None,
        }
    }
}

/// Resolver input after the scheduler selected an account and client.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResolveRequest {
    pub url: Url,
    pub client: ClientIdentity,
}

/// Header added by the trusted host after placeholder substitution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedHeader {
    pub name: String,
    pub value: String,
}

/// Optional checksum supplied by a provider.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub value: String,
}

/// Transfer URL and metadata returned by a resolver.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResolvedDownload {
    pub url: Url,
    pub file_name: Option<String>,
    pub size: Option<ByteCount>,
    pub headers: Vec<ResolvedHeader>,
    pub checksum: Option<ResolvedChecksum>,
    pub client: ClientIdentity,
}

/// One translatable part of what stands next to a provider account.
///
/// Translated like [`Failure`]: the interface looks `code` up in the active language, then in
/// English, interpolates `params`, and prints `message` only when no catalogue knows the code.
/// The host refuses a part without a code, so the text is the last rung and never a channel
/// of its own.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LabelPart {
    /// `plugin.account.*` from the core catalogue or `<provider.slug>.*` from the plugin's own.
    pub code: String,
    /// Flat parameters referenced by the translated text.
    #[serde(default, skip_serializing_if = "MessageParams::is_empty")]
    pub params: MessageParams,
    /// English, redaction-safe text for a code no catalogue translates.
    pub message: String,
}

/// Redaction-safe account state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountStatus {
    pub valid: bool,
    pub premium: bool,
    /// What the interface prints next to the account, one translated part after another.
    /// Empty when the check has nothing to add to `valid` and `premium`.
    #[serde(default)]
    pub label: Vec<LabelPart>,
    /// Remaining traffic in bytes. Providers reporting megabytes or gigabytes convert
    /// before returning; passing another unit through shows the wrong figure to the user.
    pub traffic_left: Option<ByteCount>,
}

/// Batch availability probe without downloading anything.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CheckRequest {
    pub urls: Vec<Url>,
    pub client: ClientIdentity,
}

/// Core resolver abstraction; native implementations require no WASM toolchain.
#[async_trait]
pub trait Resolver: Send + Sync {
    fn metadata(&self) -> &ResolverMetadata;
    fn matches(&self, url: &Url) -> bool;
    async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure>;
    async fn resolve(&self, request: ResolveRequest) -> Result<ResolvedDownload, Failure>;

    /// Reports availability, file name and size for several links (one result per URL).
    async fn check(&self, _request: CheckRequest) -> Result<Vec<LinkCheckResult>, Failure> {
        Err(Failure::coded(
            FailureKind::Unsupported,
            "link.check_unsupported",
            "Link check is not supported by this resolver",
        ))
    }

    /// Hoster domains the account can download from. Multihosters return their live
    /// catalogue; single hosters default to their own non-wildcard domains.
    async fn hosters(&self, _account_id: AccountId) -> Result<Vec<String>, Failure> {
        Ok(self
            .metadata()
            .domains
            .iter()
            .filter(|domain| !domain.starts_with("*.") && !domain.starts_with("api"))
            .cloned()
            .collect())
    }
}

/// Deny-by-default host capabilities shared by native and Component resolvers.
#[async_trait]
pub trait ResolverHost: Send + Sync {
    async fn http_request(
        &self,
        client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure>;

    /// Returns cookies scoped to one account and URL without exposing the stored cookie file.
    async fn cookies_get(&self, _account_id: AccountId, _url: &Url) -> Vec<(String, String)> {
        Vec::new()
    }

    async fn secret_available(&self, account_id: AccountId, reference: &str) -> bool;

    /// Runs `steps` over the credential behind `reference` and answers with the last step's
    /// output (RD-120-20).
    ///
    /// The counterpart of the substitution that expands `{{secret:<reference>}}` on the way
    /// out: there the host sends a credential the plugin named, here it computes with one.
    /// Nothing of the credential itself comes back — not the value, not an intermediate,
    /// only what the last step produced.
    ///
    /// Defaulted to a refusal, like the two storing calls above, so a host without a vault
    /// says so instead of answering with something it made up.
    async fn derive_from_secret(
        &self,
        _client: &ClientIdentity,
        _reference: &str,
        _steps: &[DerivationStep],
    ) -> Result<Vec<u8>, Failure> {
        Err(Failure::coded(
            FailureKind::Unsupported,
            "plugin.key_derivation_unsupported",
            "Deriving from a credential is not supported by this host",
        ))
    }

    /// Stores what an authentication flow produced, for the account it was running for.
    ///
    /// The caller names no vault reference. Which one belongs to the account's provider is
    /// the host's to look up, so a plugin cannot write into a reference it has no claim to,
    /// and there is no counterpart that reads the value back. Hosts that have no vault
    /// refuse, which keeps a flow from reporting success while nothing was kept.
    async fn store_token(&self, _account_id: AccountId, _value: &str) -> Result<(), Failure> {
        Err(Failure::coded(
            FailureKind::Unsupported,
            "plugin.store_token_unsupported",
            "Storing a credential is not supported by this host",
        ))
    }

    /// Stores what an OAuth exchange produced (RD-103-00).
    ///
    /// The access token goes where `store_token` puts one. What this adds is the pair that
    /// makes renewal possible at all: the material that mints the next token, and how long the
    /// current one lasts. A provider that sends refresh material only on the first exchange is
    /// the normal case, so `None` there means "keep what is already stored", not "drop it".
    ///
    /// Defaulted like its sibling: a host with no vault refuses rather than reporting a
    /// success it did not achieve.
    async fn store_oauth_token(
        &self,
        _account_id: AccountId,
        _access_token: &str,
        _refresh_token: Option<&str>,
        _expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        Err(Failure::coded(
            FailureKind::Unsupported,
            "plugin.store_token_unsupported",
            "Storing a credential is not supported by this host",
        ))
    }

    /// Waits out a hoster countdown on the host's clock. Hosts that cannot wait report
    /// `Unsupported`, which keeps a resolver from silently skipping a mandatory delay.
    async fn wait(&self, _client: &ClientIdentity, _seconds: u32) -> Result<(), Failure> {
        Err(Failure::coded(
            FailureKind::Unsupported,
            "plugin.wait_unsupported",
            "Waiting is not supported by this host",
        ))
    }

    /// Seconds since the Unix epoch: what the guest's `now-unix-seconds` answers.
    ///
    /// The host's clock, so a test host can pin it. A plugin that turns an absolute timestamp
    /// from its provider into a wait is otherwise only testable against the wall clock, and a
    /// slow runner then moves the answer (RD-120-67).
    fn now_unix_seconds(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs())
    }

    /// Longest time one captcha may occupy, so the caller can reserve that much waiting
    /// budget before handing a challenge over.
    async fn captcha_allowance(&self) -> Duration {
        DEFAULT_CAPTCHA_ALLOWANCE
    }

    /// Solves a captcha through a configured solver service or by asking the user.
    ///
    /// `time_limit` is the waiting time the caller actually reserved; answering must not
    /// take longer, or the resolver behind it outlives its own execution budget.
    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        _challenge: CaptchaChallenge,
        _time_limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        Err(Failure::coded(
            FailureKind::NeedsCaptcha,
            "captcha.no_solver",
            "No captcha solver is configured",
        ))
    }
}

/// Application-side captcha solving, injected into the resolver host.
///
/// Kept as a trait so the host depends on the capability rather than on the solver
/// implementation, and so tests can answer challenges without a service or a UI.
#[async_trait]
pub trait CaptchaSolver: Send + Sync {
    /// Waiting time one challenge may need, given the current configuration.
    async fn allowance(&self) -> Duration {
        DEFAULT_CAPTCHA_ALLOWANCE
    }

    /// Answers a challenge within `limit`, which the host has reserved for it, in the shape
    /// the challenge has.
    async fn solve(
        &self,
        challenge: CaptchaChallenge,
        limit: Duration,
    ) -> Result<CaptchaAnswer, Failure>;
}

/// WIT-shaped failure variant used by adapters and parity tests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WitFailureKind {
    Transient { retry_after_seconds: Option<u64> },
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited { retry_after_seconds: Option<u64> },
    NeedsCaptcha,
    Unsupported,
    IpBlocked { retry_after_seconds: Option<u64> },
    CaptchaFailed,
}

impl From<FailureKind> for WitFailureKind {
    fn from(value: FailureKind) -> Self {
        match value {
            FailureKind::Transient {
                retry_after_seconds,
            } => Self::Transient {
                retry_after_seconds,
            },
            FailureKind::Permanent => Self::Permanent,
            FailureKind::Offline => Self::Offline,
            FailureKind::AuthRequired => Self::AuthRequired,
            FailureKind::AccountInvalid => Self::AccountInvalid,
            FailureKind::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            FailureKind::NeedsCaptcha => Self::NeedsCaptcha,
            FailureKind::Unsupported => Self::Unsupported,
            FailureKind::IpBlocked {
                retry_after_seconds,
            } => Self::IpBlocked {
                retry_after_seconds,
            },
            FailureKind::CaptchaFailed => Self::CaptchaFailed,
        }
    }
}

impl From<WitFailureKind> for FailureKind {
    fn from(value: WitFailureKind) -> Self {
        match value {
            WitFailureKind::Transient {
                retry_after_seconds,
            } => Self::Transient {
                retry_after_seconds,
            },
            WitFailureKind::Permanent => Self::Permanent,
            WitFailureKind::Offline => Self::Offline,
            WitFailureKind::AuthRequired => Self::AuthRequired,
            WitFailureKind::AccountInvalid => Self::AccountInvalid,
            WitFailureKind::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            WitFailureKind::NeedsCaptcha => Self::NeedsCaptcha,
            WitFailureKind::Unsupported => Self::Unsupported,
            WitFailureKind::IpBlocked {
                retry_after_seconds,
            } => Self::IpBlocked {
                retry_after_seconds,
            },
            WitFailureKind::CaptchaFailed => Self::CaptchaFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use rd_core::FailureKind;

    use super::WitFailureKind;

    #[test]
    fn rust_and_wit_failure_variants_round_trip() {
        let variants = [
            FailureKind::Transient {
                retry_after_seconds: Some(12),
            },
            FailureKind::Permanent,
            FailureKind::Offline,
            FailureKind::AuthRequired,
            FailureKind::AccountInvalid,
            FailureKind::RateLimited {
                retry_after_seconds: None,
            },
            FailureKind::NeedsCaptcha,
            FailureKind::Unsupported,
            FailureKind::IpBlocked {
                retry_after_seconds: Some(900),
            },
            FailureKind::CaptchaFailed,
        ];
        for variant in variants {
            let restored = FailureKind::from(WitFailureKind::from(variant.clone()));
            assert_eq!(restored, variant);
        }
    }
}

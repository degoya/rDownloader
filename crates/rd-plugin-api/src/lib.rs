//! Native resolver trait and WebAssembly component contract types.

#![warn(unreachable_pub)]

use std::time::Duration;

use async_trait::async_trait;
use rd_plugin_types::{
    AccountId, ByteCount, ChecksumAlgorithm, Failure, FailureKind, MessageParams, PluginId,
    PluginLinkCheck, ProxyProfileId,
};
use serde::{Deserialize, Serialize};
use url::Url;

/// The scripted host the native plugin suites share; never part of the service.
#[cfg(all(feature = "test-support", not(target_arch = "wasm32")))]
pub mod test_support;

mod captcha;
mod manifest;
mod wit_failure;

pub use captcha::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolver, ClickPoint, CutcaptchaChallenge,
    ImageChallenge, WidgetChallenge,
};
pub use manifest::metadata_from_manifest;
pub use wit_failure::WitFailureKind;

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
    async fn check(&self, _request: CheckRequest) -> Result<Vec<PluginLinkCheck>, Failure> {
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

    /// Stores one named part of what a sign-in produced, beside its token (RD-150-09).
    ///
    /// `name` is a slot the account's provider declares as filled by the flow; the host checks
    /// that and keeps the part as its own vault entry. Like its siblings there is no way to
    /// read it back, and a host without a vault refuses.
    async fn store_flow_secret(
        &self,
        _account_id: AccountId,
        _name: &str,
        _value: &str,
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

#[cfg(test)]
mod tests {
    use rd_plugin_types::FailureKind;

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

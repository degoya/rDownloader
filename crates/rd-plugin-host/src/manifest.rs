//! Manifest v3: the complete, self-describing plugin contract.
//!
//! A v3 manifest carries everything the core needs to run a plugin it has never seen
//! before: identity and authorship (`[metadata]`), the plugin type and the ABI version it
//! speaks, the capabilities it is granted (`[capabilities]`), the provider it serves
//! (`[provider]`, mirrored into `rd_provider_registry` at startup), and the signing key the
//! author used. Nothing about a third-party plugin is hardcoded in the core, so a hosting
//! provider can ship a resolver without a core release.
//!
//! Every grant is one entry in `[capabilities]` and one WIT interface in the linker. A
//! plugin therefore reaches exactly what its manifest names, the plugin manager can show
//! that same list, and a capability this core does not know is refused instead of ignored.

use anyhow::{Context, Result, bail};
use ed25519_dalek::VerifyingKey;
use rd_core::PluginId;
use serde::{Deserialize, Serialize};

use crate::{DEFAULT_FUEL, PluginLimits};

mod checks;
mod sections;
mod validate;

pub use sections::{
    Capabilities, CredentialKindManifest, CredentialModeManifest, ExtensionManifest,
    NetHttpCapability, NetStreamCapability, OAuthFlowManifest, PluginMetadata,
    ProviderKindManifest, ProviderManifest, REMOTE_JOB_CONTAINERS, SecretFilledByManifest,
    SecretSlotManifest, SettingManifest, TransferAuthManifest, TransferManifest,
};
pub(crate) use validate::validate_manifest;

/// The only manifest revision this core accepts.
pub const MANIFEST_VERSION: u32 = 3;

/// WIT package versions of `rdownloader:plugin` this core can link a plugin against.
pub const SUPPORTED_API_VERSIONS: &[&str] = &["0.10.0"];

/// A manifest this core refuses, in the two shapes a user can act on.
///
/// Everything else stays an opaque `anyhow` chain: those are packaging mistakes an author
/// fixes. These two reach the plugin manager, where the difference matters — an outdated
/// package needs an update, an unknown grant is a package this build cannot run at all.
#[derive(Debug, thiserror::Error)]
pub enum ManifestRejection {
    /// Written for a different manifest revision; `plugin.manifest_outdated`.
    #[error("unsupported manifest version {found} (expected {expected})")]
    Version { found: u32, expected: u32 },
    /// Names a plugin type, capability or ABI version this build does not know;
    /// `plugin.capability_unknown`.
    #[error("unknown {kind} `{value}`")]
    Unknown { kind: &'static str, value: String },
}

impl ManifestRejection {
    /// Stable translation code the API and UI switch on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Version { .. } => "plugin.manifest_outdated",
            Self::Unknown { .. } => "plugin.capability_unknown",
        }
    }
}

/// The fields every manifest revision carries, read from a package this build refuses.
///
/// A refused package still has to be recognisable: without this the plugin manager would
/// simply stop listing a third-party resolver after an upgrade, and nobody could tell a
/// package that needs rebuilding from one that was never installed.
#[derive(Clone, Debug, Deserialize)]
pub struct ManifestHeader {
    pub manifest_version: u32,
    pub id: PluginId,
    pub name: String,
    pub version: String,
}

/// What a plugin is. The type decides which world it is instantiated against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginType {
    /// Resolves hoster URLs and checks links (`world resolver-plugin`).
    Resolver,
    /// Carries bytes for a transfer protocol (`world transfer-plugin`).
    Transfer,
    /// Turns raw text and URLs into LinkGrabber candidates (`world intake-plugin`).
    Intake,
    /// Runs a provider authentication flow (`world auth-plugin`).
    Auth,
    /// Runs an OAuth redirect flow and renews its token (`world oauth-plugin`).
    OAuth,
    /// Turns one address into the files behind it (`world crawler-plugin`).
    Crawler,
    /// Adds metadata to a link or a queued item (`world enricher-plugin`).
    Enricher,
    /// Delivers notifications to another destination (`world notifier-plugin`).
    Notifier,
    /// Runs one post-processing step (`world postprocess-plugin`).
    Postprocess,
    /// Uploads finished files to a storage destination (`world storage-plugin`).
    Storage,
    /// Runs a job that lives at the provider and outlives the call that started it
    /// (`world remote-job-plugin`, RD-107-06). See
    /// `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
    RemoteJob,
    /// Answers with an address and how the bytes behind it become a file, for a provider
    /// that encrypts on the client (`world stream-transform-plugin`, RD-110-33). See
    /// `docs/adr/0011-mega-a-stream-the-host-must-decrypt.md`.
    StreamTransform,
    /// Anything this build does not know; refused by [`validate_manifest`] rather than
    /// silently treated as a resolver.
    Unknown(String),
}

impl PluginType {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Resolver => "resolver",
            Self::Transfer => "transfer",
            Self::Intake => "intake",
            Self::Auth => "auth",
            Self::OAuth => "oauth",
            Self::Crawler => "crawler",
            Self::Enricher => "enricher",
            Self::Notifier => "notifier",
            Self::Postprocess => "postprocess",
            Self::Storage => "storage",
            Self::RemoteJob => "remote-job",
            Self::StreamTransform => "stream-transform",
            Self::Unknown(value) => value,
        }
    }
}

impl serde::Serialize for PluginType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for PluginType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "resolver" => Self::Resolver,
            "transfer" => Self::Transfer,
            "intake" => Self::Intake,
            "auth" => Self::Auth,
            "oauth" => Self::OAuth,
            "crawler" => Self::Crawler,
            "enricher" => Self::Enricher,
            "notifier" => Self::Notifier,
            "postprocess" => Self::Postprocess,
            "storage" => Self::Storage,
            "remote-job" => Self::RemoteJob,
            "stream-transform" => Self::StreamTransform,
            _ => Self::Unknown(raw),
        })
    }
}

const MAX_DESCRIPTION_CHARS: usize = 500;
const MAX_AUTHOR_CHARS: usize = 120;
const MAX_LICENSE_CHARS: usize = 64;
const MAX_URL_CHARS: usize = 256;
const MAX_SLUG_CHARS: usize = 32;
/// Hard ceiling for a plugin's declared waiting time (30 minutes).
const MAX_WAIT_BUDGET_MILLISECONDS: u64 = 30 * 60 * 1000;
/// Hard ceilings for the rest of the sandbox budget a manifest declares for itself.
///
/// The limits in a manifest are the plugin's own request, and the manifest is written by
/// whoever wrote the plugin. Refusing only zero left the fuel, memory and timeout guards
/// self-granted: a third-party manifest asking for 8 GiB, `u64::MAX` fuel and a day-long
/// epoch deadline got exactly that, and the sandbox then bounded nothing. A plugin may ask
/// for less than the ceiling and is refused above it.
const MAX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FUEL: u64 = 20 * DEFAULT_FUEL;
const MAX_TIMEOUT_MILLISECONDS: u64 = MAX_WAIT_BUDGET_MILLISECONDS;
pub(crate) const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MIN_SLUG_CHARS: usize = 2;

/// Signed metadata included in a `.rdplug` archive.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginManifest {
    /// Manifest revision; must equal [`MANIFEST_VERSION`].
    pub manifest_version: u32,
    /// What this plugin is; decides which world it is instantiated against.
    pub plugin_type: PluginType,
    /// Version of the `rdownloader:plugin` WIT package the component was built against.
    pub api_version: String,
    pub id: PluginId,
    /// Default display name. Localised overrides live in `locales/<lang>.json`.
    pub name: String,
    pub version: String,
    /// Names the signing key; also the trust-store key for this author.
    pub key_id: String,
    /// Base64 Ed25519 public key of `key_id`, used for trust-on-first-use.
    pub public_key: String,
    pub metadata: PluginMetadata,
    /// The provider row a resolver contributes. Required for `plugin_type = "resolver"` and
    /// refused for every other type: a transfer backend serves URL schemes, not an account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderManifest>,
    /// The schemes a transfer backend claims. Required for `plugin_type = "transfer"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<TransferManifest>,
    /// The declaration of every plugin type beyond resolver and transfer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<ExtensionManifest>,
    /// Which ways in an `oauth` plugin serves, in the order it prefers them (RD-106-01).
    ///
    /// Every plugin of that world exports all four calls, because a world is all or nothing.
    /// What it *serves* is a different question, and it is the plugin author's to answer:
    /// most providers offer a redirect or a device code, not both. Stating it here means the
    /// host never calls an entrance nobody implemented, and nobody has to write a stub whose
    /// only job is to be refused.
    ///
    /// Absent means `["redirect"]`, which is what every manifest written before this field
    /// existed meant. Refused on any other plugin type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oauth_flows: Vec<OAuthFlowManifest>,
    /// Every grant this plugin asks for, including the HTTP domain allowlist.
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub match_domains: Vec<String>,
    #[serde(default)]
    pub download_domains: Vec<String>,
    /// Hosts whose link fragment carries key material rather than an anchor (RD-110-38).
    ///
    /// The declaration that keeps two accepted decisions from colliding. RD-109-32 drops the
    /// fragment of every address before a candidate row exists, because nothing distinguishes
    /// a share password from an anchor name; a provider that encrypts on the client puts the
    /// file key in exactly that fragment (ADR 0011). Naming the hosts here is what tells the
    /// intake to put the fragment in the vault instead of throwing it away -- and without it
    /// nothing changes, so no service is a special case in the code.
    ///
    /// Same pattern language as `match_domains`: an exact host, or `*.suffix` for its
    /// sub-domains. Declared per host rather than per world on purpose: a crawler that emits
    /// child links of the same provider needs no declaration of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_fragment_domains: Vec<String>,
    /// How many downloads this plugin may run at once.
    ///
    /// Defaulted rather than required since the extension types arrived: an intake parser or
    /// a notification destination has no downloads to limit, and forcing every such manifest
    /// to state a number would be asking authors to write something that means nothing.
    #[serde(default = "default_concurrency")]
    pub max_concurrent_downloads: u32,
    /// Whether resolving requires a provider account; `false` opts the plugin into
    /// account-less (free) resolve calls. Absent in older manifests, so it defaults to `true`.
    #[serde(default = "default_requires_account")]
    pub requires_account: bool,
    #[serde(default)]
    pub limits: PluginLimits,
}

fn default_requires_account() -> bool {
    true
}

impl PluginManifest {
    /// Namespace every translated message and failure code of this plugin starts with.
    ///
    /// A resolver namespaces on the provider it serves, a transfer backend on the schemes it
    /// claims; both need exactly one stable prefix, so they share the accessor rather than the
    /// field.
    #[must_use]
    pub fn message_slug(&self) -> &str {
        match (&self.provider, &self.transfer, &self.extension) {
            (Some(provider), _, _) => &provider.slug,
            (_, Some(transfer), _) => &transfer.slug,
            (_, _, Some(extension)) => &extension.slug,
            _ => "",
        }
    }

    /// HTTP sandbox allowlist, owned by the `net_http` capability.
    #[must_use]
    pub fn domains(&self) -> &[String] {
        self.capabilities.domains()
    }

    /// The ways in this plugin serves, in the order it prefers them (RD-106-01).
    ///
    /// A manifest that says nothing means the redirect, which is what every `oauth` manifest
    /// written before the field existed meant. The order is the author's preference: a
    /// provider offering both gets its first entry, so the choice is stated in the package
    /// the person installed rather than guessed at by the host.
    #[must_use]
    pub fn oauth_flows(&self) -> &[OAuthFlowManifest] {
        if self.oauth_flows.is_empty() {
            &[OAuthFlowManifest::Redirect]
        } else {
            &self.oauth_flows
        }
    }

    /// Whether this plugin serves the given way in.
    #[must_use]
    pub fn serves_oauth_flow(&self, flow: OAuthFlowManifest) -> bool {
        self.oauth_flows().contains(&flow)
    }

    /// Decodes the author's Ed25519 verification key.
    pub fn verifying_key(&self) -> Result<VerifyingKey> {
        decode_public_key(&self.public_key)
    }

    /// Hosts this provider claims plain download URLs for, derived from `match_domains`.
    ///
    /// Wildcard entries (`*`, `*.suffix`) are intake patterns for multihosters, not
    /// concrete hosts, so they never contribute a match host.
    #[must_use]
    pub fn match_hosts(&self) -> Vec<String> {
        self.match_domains
            .iter()
            .filter(|domain| !domain.contains('*'))
            .cloned()
            .collect()
    }
}

/// Decodes a base64 32-byte Ed25519 public key.
pub use rd_sign::decode_public_key;

/// Rejects `.`, `..` and separators so a manifest value can name a directory.
pub(crate) fn safe_segment(value: &str) -> Result<&str> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\']) {
        bail!("unsafe plugin version path");
    }
    Ok(value)
}

/// Keeps the grant list and the provider declaration from drifting apart.
///
/// The provider fields describe *what* a credential or cookie is for; the capability list
/// is *whether* the plugin may touch it at all. Both have to agree, or the plugin manager
/// would show a grant the runtime does not enforce — or worse, the other way round.
/// Each plugin type carries exactly the section that describes it, and not the other one.
///
/// A transfer backend with a `[provider]` block would contribute a provider row nothing
/// serves; a resolver with `[transfer]` would claim schemes nothing routes. Both are silent
/// misconfiguration, so both are refused.
/// One concurrent download, which is what a plugin that runs none needs to say.
const fn default_concurrency() -> u32 {
    1
}

/// Derives the registry row this plugin contributes.
///
/// The manifest is the sole authority: the sandbox allowlist (`domains`) becomes the
/// provider's `request_domains`, so a third-party plugin reaches its own hosts and only
/// its own. Nothing here consults the built-in table.
#[must_use]
pub fn provider_spec_from_manifest(
    manifest: &PluginManifest,
) -> Option<rd_provider_registry::DynamicProvider> {
    let provider = manifest.provider.as_ref()?;
    Some(rd_provider_registry::DynamicProvider {
        plugin_id: manifest.id.to_string(),
        spec: rd_provider_registry::ProviderSpec {
            slug: provider.slug.clone(),
            display_name: manifest.name.clone(),
            kind: match provider.kind {
                ProviderKindManifest::Hoster => rd_provider_registry::ProviderKind::Hoster,
                ProviderKindManifest::Multihoster => {
                    rd_provider_registry::ProviderKind::Multihoster
                }
            },
            credentials: match provider.credentials {
                CredentialKindManifest::NoneRequired => {
                    rd_provider_registry::CredentialKind::NoneRequired
                }
                CredentialKindManifest::ApiKey => rd_provider_registry::CredentialKind::ApiKey,
                CredentialKindManifest::UsernamePassword => {
                    rd_provider_registry::CredentialKind::UsernamePassword
                }
                CredentialKindManifest::ApiKeyOrCookies => {
                    rd_provider_registry::CredentialKind::ApiKeyOrCookies
                }
                CredentialKindManifest::Cookies => rd_provider_registry::CredentialKind::Cookies,
                CredentialKindManifest::LoginOrApiKey => {
                    rd_provider_registry::CredentialKind::LoginOrApiKey
                }
                CredentialKindManifest::OAuth => rd_provider_registry::CredentialKind::OAuth,
                CredentialKindManifest::OAuthOrApiKey => {
                    rd_provider_registry::CredentialKind::OAuthOrApiKey
                }
            },
            username_required: provider.username_required,
            transfer_auth: match provider.transfer_auth {
                TransferAuthManifest::None => rd_provider_registry::TransferAuth::None,
                TransferAuthManifest::Basic => rd_provider_registry::TransferAuth::Basic,
            },
            secrets: provider
                .secret_slots()
                .into_iter()
                .map(|slot| rd_provider_registry::SecretSlot {
                    reference: slot.reference,
                    domains: slot.domains,
                    mode: slot.mode.map(|mode| match mode {
                        CredentialModeManifest::Login => {
                            rd_provider_registry::CredentialMode::Login
                        }
                        CredentialModeManifest::ApiKey => {
                            rd_provider_registry::CredentialMode::ApiKey
                        }
                        CredentialModeManifest::OAuth => {
                            rd_provider_registry::CredentialMode::OAuth
                        }
                    }),
                    filled_by: match slot.filled_by {
                        SecretFilledByManifest::Person => {
                            rd_provider_registry::SecretFilledBy::Person
                        }
                        SecretFilledByManifest::Flow => rd_provider_registry::SecretFilledBy::Flow,
                    },
                })
                .collect(),
            request_domains: manifest.domains().to_vec(),
            cookie_scope: provider.cookie_scope.clone(),
            match_hosts: match provider.kind {
                ProviderKindManifest::Hoster => manifest.match_hosts(),
                // A multihoster resolves other hosters' links; it never claims a URL itself.
                ProviderKindManifest::Multihoster => Vec::new(),
            },
            host_aliases: provider.host_aliases.clone(),
            source: rd_provider_registry::ProviderSource::Plugin,
            plugin_id: Some(manifest.id.to_string()),
            plugin_version: Some(manifest.version.clone()),
        },
    })
}

/// Refuses a plugin that needs a newer core than this build.
pub fn check_app_version(manifest: &PluginManifest, app_version: &str) -> Result<()> {
    let Some(required) = &manifest.metadata.min_app_version else {
        return Ok(());
    };
    let required = semver::Version::parse(required)
        .context("metadata.min_app_version is not semantic versioning")?;
    let current =
        semver::Version::parse(app_version).context("application version is not semantic")?;
    if current < required {
        bail!("plugin requires rDownloader {required} or newer (running {current})");
    }
    Ok(())
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;

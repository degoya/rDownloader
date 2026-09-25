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

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use ed25519_dalek::VerifyingKey;
use rd_core::PluginId;
use serde::{Deserialize, Serialize};

use crate::{DEFAULT_FUEL, PluginLimits};

/// The only manifest revision this core accepts.
pub const MANIFEST_VERSION: u32 = 3;

/// WIT package versions of `rdownloader:plugin` this core can link a plugin against.
pub const SUPPORTED_API_VERSIONS: &[&str] = &["0.9.0"];

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

/// Outbound HTTPS, confined to the listed domains.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NetHttpCapability {
    /// The sandbox allowlist: the authority for this plugin's outbound requests.
    pub domains: Vec<String>,
}

/// Raw TCP/TLS, confined to named hosts and ports.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NetStreamCapability {
    /// Hosts this backend may connect to; same pattern language as `net_http.domains`.
    pub hosts: Vec<String>,
    /// Ports it may connect to. There is no wildcard: a backend that may reach any port on a
    /// host it named is a port scanner with a manifest.
    pub ports: Vec<u16>,
}

/// The grants a manifest asks for. Each one maps to exactly one WIT interface.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Capabilities {
    /// `rdownloader:plugin/http`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_http: Option<NetHttpCapability>,
    /// `rdownloader:plugin/cookies`.
    #[serde(default)]
    pub cookies: bool,
    /// `rdownloader:plugin/captcha`.
    #[serde(default)]
    pub captcha: bool,
    /// `rdownloader:plugin/net`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_stream: Option<NetStreamCapability>,
    /// `{{secret:<reference>}}` markers this plugin may expand.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// `rdownloader:plugin/key-derivation` (RD-120-20).
    ///
    /// A plugin that does not declare it does not get the interface linked, so a component
    /// that imports it without saying so fails to instantiate. Declaring it is not a way to
    /// reach a new credential: the references it may compute over are exactly the ones
    /// `secrets` already lists.
    #[serde(default)]
    pub key_derivation: bool,
    /// Keys this build does not know. Captured rather than ignored so an unknown grant is
    /// refused with a clear message instead of silently doing nothing.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, toml::Value>,
}

impl Capabilities {
    /// Domains the plugin may reach; empty when it has no network grant at all.
    #[must_use]
    pub fn domains(&self) -> &[String] {
        self.net_http.as_ref().map_or(&[], |http| &http.domains)
    }

    /// The grants as the plugin manager lists them, in a stable order.
    #[must_use]
    pub fn granted(&self) -> Vec<String> {
        let mut granted = Vec::new();
        if self.net_http.is_some() {
            granted.push("net_http".to_owned());
        }
        if self.cookies {
            granted.push("cookies".to_owned());
        }
        if self.captcha {
            granted.push("captcha".to_owned());
        }
        if let Some(stream) = &self.net_stream {
            granted.push(format!(
                "net_stream:{}",
                stream
                    .ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        for reference in &self.secrets {
            granted.push(format!("secrets:{reference}"));
        }
        if self.key_derivation {
            granted.push("key_derivation".to_owned());
        }
        granted
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
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
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

/// Authorship and presentation details shown in the plugin manager.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginMetadata {
    /// Default description. Localised overrides live in `locales/<lang>.json`.
    pub description: String,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_url: Option<String>,
    /// SPDX licence identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Lowest core version this plugin runs on; installation is refused below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_app_version: Option<String>,
}

/// What a transfer backend claims and how its messages are namespaced.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TransferManifest {
    /// Message namespace, the same role `provider.slug` plays for a resolver.
    pub slug: String,
    /// URL schemes this backend handles, lowercase and without `://`.
    pub schemes: Vec<String>,
}

/// The section every plugin type beyond resolver and transfer declares.
///
/// One shared shape rather than six near-identical ones: what these types have in common is
/// a message namespace and, for the ones the core has to route to, a list of what they
/// claim. A type that needs nothing more than a slug simply leaves `claims` empty.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExtensionManifest {
    /// Message namespace, the same role `provider.slug` plays for a resolver.
    pub slug: String,
    /// What this plugin claims, interpreted per type: intake and enricher read it as domain
    /// patterns, auth as provider slugs, notifier as destination kinds. Empty means the
    /// plugin is offered for everything of its type.
    #[serde(default)]
    pub claims: Vec<String>,
    /// A crawler that recognises an address by the shape of its path rather than by its host,
    /// and is therefore asked only after every crawler that named a service (RD-107-05).
    ///
    /// `GenericHTTPDirectoryIndexCrawler` is the whole reason this field exists: a plugin
    /// that claims "any address ending in a slash" would otherwise win the address of a
    /// service whose own crawler was installed, purely by being earlier in the list.
    #[serde(default)]
    pub generic: bool,
}

/// One way into an `oauth` plugin (RD-106-01).
///
/// Spelled out rather than inferred, because guessing would mean calling an entrance to find
/// out whether it exists — and the answer to that question is a failed sign-in the person sees.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowManifest {
    /// The person agrees in a browser and the provider redirects back with a code.
    Redirect,
    /// The person types a short code on another screen; nothing redirects anywhere.
    Device,
}

impl OAuthFlowManifest {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Redirect => "redirect",
            Self::Device => "device",
        }
    }
}

/// Whether a provider resolves its own domains or other hosters' on the account's behalf.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKindManifest {
    Hoster,
    Multihoster,
}

/// The shape of the credential(s) a provider account stores.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKindManifest {
    ApiKey,
    UsernamePassword,
    ApiKeyOrCookies,
    Cookies,
    /// Two mutually exclusive ways to hold one account — sign in with username and password,
    /// or paste a ready-made API key — chosen per account. A provider declaring this must
    /// describe both in `[[provider.secrets]]`, one slot per mode, so that each credential
    /// stays pinned to the hosts its own mode talks to.
    LoginOrApiKey,
    /// Signed in through an OAuth redirect, renewed from stored refresh material. Written
    /// `credentials = "oauth"` in a manifest; the flow itself belongs to an `oauth` plugin.
    ///
    /// Renamed explicitly: `rename_all = "snake_case"` would spell this `o_auth`, and no
    /// manifest author would ever guess that.
    #[serde(rename = "oauth")]
    OAuth,
    /// The provider takes no account at all: it resolves the free flow and nothing else.
    ///
    /// Written `credentials = "none"` in a manifest. Spelled `NoneRequired` here rather than
    /// `None` so that a match arm in a file full of `Option` says which `None` it means.
    /// A resolver spanning several sites of the same hosting script needs this: it can name
    /// neither one `cookie_scope` nor one `secret_reference`, because an account at one clone
    /// is not an account at another (RD-098-01).
    #[serde(rename = "none")]
    NoneRequired,
}

/// How the account's credential travels on a transfer; `transfer_auth` in `[provider]`
/// (RD-120-38). See [`rd_provider_registry::TransferAuth`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferAuthManifest {
    /// Absent, or written `transfer_auth = "none"`: the download engine attaches nothing.
    #[default]
    None,
    /// `transfer_auth = "basic"`: the engine sends `Authorization: Basic` built from the
    /// account's username and its one secret, to that secret's `secret_domains` only.
    Basic,
}

fn is_no_transfer_auth(value: &TransferAuthManifest) -> bool {
    *value == TransferAuthManifest::None
}

/// Which credential mode a `[[provider.secrets]]` slot belongs to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialModeManifest {
    Login,
    ApiKey,
}

/// Who puts a value into a credential slot (RD-106-03).
///
/// Almost every slot is filled by the person, which is why that is the default and why nine
/// manifests written before this existed keep meaning what they meant. The exception is an
/// OAuth provider where the person registers their own application: they supply the client
/// secret, the sign-in obtains the access token, and both have to live at once. One value can
/// only be stored in one place, so the two need separate slots -- and the host has to be told
/// which is which rather than inferring it from the order they happen to be written in.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretFilledByManifest {
    /// The person types it into the accounts form. The account's own credential.
    #[default]
    Person,
    /// A sign-in flow obtains it and the host stores it beside the flow. Nobody types it, and
    /// it must not overwrite what the person typed.
    Flow,
}

/// One credential slot of a provider, written as a `[[provider.secrets]]` table.
///
/// The singular `secret_reference` + `secret_domains` spelling stays valid and means exactly
/// one slot with no mode; a provider only needs this longer form once it offers a choice.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SecretSlotManifest {
    /// The `{{secret:<reference>}}` marker this slot answers to; must start with `<slug>_`.
    pub reference: String,
    /// Exact hosts this slot's credential may be sent to; each must be covered by `domains`.
    #[serde(default)]
    pub domains: Vec<String>,
    /// The mode that activates this slot. Required once a provider declares more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<CredentialModeManifest>,
    /// Who fills this slot. Absent means the person, which is what every slot meant before
    /// RD-106-03.
    #[serde(default, skip_serializing_if = "is_person")]
    pub filled_by: SecretFilledByManifest,
}

fn is_person(filled_by: &SecretFilledByManifest) -> bool {
    *filled_by == SecretFilledByManifest::Person
}

/// The provider row this plugin contributes to the registry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderManifest {
    /// Stored as `accounts.provider`; `^[a-z0-9_]{2,32}$`.
    pub slug: String,
    pub kind: ProviderKindManifest,
    pub credentials: CredentialKindManifest,
    #[serde(default)]
    pub username_required: bool,
    /// Whether the download engine attaches the account's credential to a transfer, and how.
    /// Absent means it does not, which is what every manifest before RD-120-38 meant.
    #[serde(default, skip_serializing_if = "is_no_transfer_auth")]
    pub transfer_auth: TransferAuthManifest,
    /// The `{{secret:<reference>}}` marker this resolver may expand; must start with `<slug>_`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_reference: Option<String>,
    /// Exact hosts the credential may be sent to; each must be covered by `domains`.
    #[serde(default)]
    pub secret_domains: Vec<String>,
    /// The provider's credential slots, for a provider that offers more than one way to sign
    /// in. Mutually exclusive with the singular `secret_reference`/`secret_domains` spelling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretSlotManifest>,
    /// Base URL whose domain receives the account's cookies; https, host covered by `domains`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_scope: Option<String>,
    /// `(alias_host, canonical_host)` pairs rewritten to this provider at link intake.
    #[serde(default)]
    pub host_aliases: Vec<(String, String)>,
}

impl ProviderManifest {
    /// The provider's credential slots, whichever spelling the manifest used.
    ///
    /// The singular `secret_reference`/`secret_domains` pair desugars to a single slot with no
    /// mode, so every consumer sees one shape and the nine manifests written before credential
    /// modes existed keep parsing unchanged.
    #[must_use]
    pub fn secret_slots(&self) -> Vec<SecretSlotManifest> {
        if !self.secrets.is_empty() {
            return self.secrets.clone();
        }
        self.secret_reference
            .iter()
            .map(|reference| SecretSlotManifest {
                reference: reference.clone(),
                domains: self.secret_domains.clone(),
                mode: None,
                filled_by: SecretFilledByManifest::Person,
            })
            .collect()
    }
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

pub(crate) fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    // The three compatibility gates come first: a package this build cannot run at all
    // must say so before anything else it might also get wrong.
    if manifest.manifest_version != MANIFEST_VERSION {
        return Err(ManifestRejection::Version {
            found: manifest.manifest_version,
            expected: MANIFEST_VERSION,
        }
        .into());
    }
    if let PluginType::Unknown(value) = &manifest.plugin_type {
        return Err(ManifestRejection::Unknown {
            kind: "plugin_type",
            value: value.clone(),
        }
        .into());
    }
    if !SUPPORTED_API_VERSIONS.contains(&manifest.api_version.as_str()) {
        return Err(ManifestRejection::Unknown {
            kind: "api_version",
            value: manifest.api_version.clone(),
        }
        .into());
    }
    if let Some(capability) = manifest.capabilities.unknown.keys().next() {
        return Err(ManifestRejection::Unknown {
            kind: "capability",
            value: capability.clone(),
        }
        .into());
    }
    safe_segment(&manifest.version)?;
    semver::Version::parse(&manifest.version)
        .context("plugin version is not semantic versioning")?;
    if manifest.name.trim().is_empty() {
        bail!("plugin name is required");
    }
    // A resolver without an HTTP allowlist could not fetch the page it resolves; a transfer
    // backend may legitimately speak nothing but its own protocol, and `validate_shape`
    // requires `net_stream` from it instead.
    if manifest.plugin_type == PluginType::Resolver && manifest.domains().is_empty() {
        bail!("a resolver needs at least one capabilities.net_http domain");
    }
    if manifest.key_id.trim().is_empty() {
        bail!("plugin key_id is required");
    }
    manifest
        .verifying_key()
        .context("manifest public_key is not a valid Ed25519 key")?;
    if manifest.max_concurrent_downloads == 0 {
        bail!("plugin concurrency limit must be greater than zero");
    }
    // `*` in the sandbox allowlist is honest only where the host narrows the host per call:
    // a storage destination, a crawler target and a notification destination are addresses
    // the person configured, and the host cuts the allowlist down to that one host
    // (`ExtensionRuntime::reachable(only_host)`, `extension::notifier::destination_reach`,
    // RD-130-15). For every other type the allowlist *is* the boundary, so a catch-all
    // removes it.
    let wildcard_allowed = matches!(
        manifest.plugin_type,
        PluginType::Storage | PluginType::Crawler | PluginType::Notifier
    );
    for domain in manifest.domains() {
        validate_domain_pattern(domain, wildcard_allowed)?;
    }
    for domain in manifest
        .match_domains
        .iter()
        .chain(&manifest.download_domains)
    {
        validate_domain_pattern(domain, true)?;
    }
    // A bare `*` here would turn every fragment in the world into vaulted key material, so
    // the catch-all is refused where it is allowed for a match list.
    for domain in &manifest.secret_fragment_domains {
        validate_domain_pattern(domain, false)?;
    }
    validate_shape(manifest)?;
    validate_capabilities(&manifest.capabilities, manifest.provider.as_ref())?;
    if manifest.limits.memory_bytes == 0
        || manifest.limits.fuel == 0
        || manifest.limits.timeout_milliseconds == 0
        || manifest.limits.max_response_bytes == 0
    {
        bail!("plugin limits must be non-zero");
    }
    for (name, value, ceiling) in [
        (
            "limits.memory_bytes",
            manifest.limits.memory_bytes,
            MAX_MEMORY_BYTES,
        ),
        ("limits.fuel", manifest.limits.fuel, MAX_FUEL),
        (
            "limits.timeout_milliseconds",
            manifest.limits.timeout_milliseconds,
            MAX_TIMEOUT_MILLISECONDS,
        ),
        (
            "limits.max_response_bytes",
            manifest.limits.max_response_bytes,
            MAX_RESPONSE_BYTES,
        ),
    ] {
        if value > ceiling {
            bail!("{name} must not exceed {ceiling}");
        }
    }
    // A wait occupies a download slot, so a plugin must not be able to park one for hours.
    if manifest.limits.wait_budget_milliseconds > MAX_WAIT_BUDGET_MILLISECONDS {
        bail!("limits.wait_budget_milliseconds must not exceed {MAX_WAIT_BUDGET_MILLISECONDS} ms");
    }
    validate_metadata(&manifest.metadata)?;
    if let Some(provider) = &manifest.provider {
        validate_provider(provider, manifest.domains())?;
    }
    Ok(())
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

fn validate_shape(manifest: &PluginManifest) -> Result<()> {
    match manifest.plugin_type {
        PluginType::Resolver => {
            if manifest.provider.is_none() {
                bail!("a resolver manifest needs a [provider] section");
            }
            if manifest.transfer.is_some() {
                bail!("a resolver manifest must not declare [transfer]");
            }
        }
        PluginType::Transfer => {
            let Some(transfer) = &manifest.transfer else {
                bail!("a transfer manifest needs a [transfer] section");
            };
            if manifest.provider.is_some() {
                bail!("a transfer manifest must not declare [provider]");
            }
            validate_slug(&transfer.slug)?;
            if transfer.schemes.is_empty() {
                bail!("transfer.schemes must name at least one scheme");
            }
            for scheme in &transfer.schemes {
                if scheme.is_empty()
                    || scheme != &scheme.to_ascii_lowercase()
                    || !scheme
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'-')
                {
                    bail!("transfer scheme `{scheme}` is not a plain lowercase scheme name");
                }
            }
            if manifest.capabilities.net_stream.is_none() {
                bail!("a transfer backend needs the net_stream capability");
            }
        }
        // Every type beyond the first two shares one shape: an `[extension]` section with a
        // slug, and neither of the two older sections. Validated together rather than six
        // times over, so a type added later cannot forget one of the two refusals.
        PluginType::Intake
        | PluginType::Auth
        | PluginType::OAuth
        | PluginType::Crawler
        | PluginType::Enricher
        | PluginType::Notifier
        | PluginType::Postprocess
        | PluginType::Storage
        | PluginType::RemoteJob
        | PluginType::StreamTransform => {
            let Some(extension) = &manifest.extension else {
                bail!(
                    "a {} manifest needs an [extension] section",
                    manifest.plugin_type.as_str()
                );
            };
            // One exception, and it is about what a plugin *is* rather than which world it
            // exports (RD-120-20). A stream-transform plugin is a resolver in everything but
            // that: it claims addresses, it talks to the provider's API, and it answers with
            // the address a download runs on. It exports the twelfth world only because the
            // provider encrypts on the client and the bytes have to be transformed on the
            // host's write path (ADR 0011). Such a plugin is therefore the natural owner of
            // its provider's account row -- MEGA had none at all, and no account could be
            // configured for it, because the only two manifests that named MEGA were an
            // `[extension]` apiece.
            //
            // The sign-in was tried as the owner first and is the wrong one, for a reason
            // this file decides: `message_slug` is the *provider* slug when a manifest has
            // one, so a `[provider]` on `plugins/mega-auth` would have moved its codes into
            // `mega.*`, which `plugins/mega` already owns thirteen of. One namespace, one
            // owner.
            if manifest.transfer.is_some()
                || (manifest.provider.is_some()
                    && manifest.plugin_type != PluginType::StreamTransform)
            {
                bail!(
                    "a {} manifest must not declare [provider] or [transfer]",
                    manifest.plugin_type.as_str()
                );
            }
            validate_slug(&extension.slug)?;
            for claim in &extension.claims {
                bounded_text("extension.claims entry", claim, MAX_SLUG_CHARS)?;
            }
            // Which ways in an OAuth plugin serves. Only that type has any, and a duplicate
            // entry would make the preference order meaningless.
            if manifest.plugin_type == PluginType::OAuth {
                let mut seen = Vec::new();
                for flow in &manifest.oauth_flows {
                    if seen.contains(flow) {
                        bail!("oauth_flows names `{}` twice", flow.as_str());
                    }
                    seen.push(*flow);
                }
            }
            // A storage destination writes somewhere; without an outbound grant it could
            // not, and a manifest that asks for neither is a mistake rather than a plugin
            // that uploads to nowhere.
            if manifest.plugin_type == PluginType::Storage
                && manifest.capabilities.net_http.is_none()
                && manifest.capabilities.net_stream.is_none()
            {
                bail!("a storage destination needs the net_http or net_stream capability");
            }
            // A crawler fetches the folder it was asked to open; without an outbound
            // grant it would answer "empty" to every address it claims, which is worse
            // than refusing the manifest.
            if manifest.plugin_type == PluginType::Crawler
                && manifest.capabilities.net_http.is_none()
            {
                bail!("a crawler needs at least one capabilities.net_http domain");
            }
            // Same rule, same reason, for the eleventh type (RD-107-06): a remote job that
            // cannot reach its provider cannot submit, poll, choose or delete anything. A
            // manifest asking for no way out is a mistake and not a plugin that submits to
            // nowhere.
            if manifest.plugin_type == PluginType::RemoteJob
                && manifest.capabilities.net_http.is_none()
            {
                bail!("a remote-job plugin needs at least one capabilities.net_http domain");
            }
            // And the twelfth (RD-110-33): a plugin that cannot reach its provider cannot
            // learn the address or the key schedule it exists to answer with.
            if manifest.plugin_type == PluginType::StreamTransform
                && manifest.capabilities.net_http.is_none()
            {
                bail!("a stream-transform plugin needs at least one capabilities.net_http domain");
            }
            // Only a crawler is ever asked in an order, so on any other type the flag would
            // be a claim about behaviour that does not exist.
            if extension.generic && manifest.plugin_type != PluginType::Crawler {
                bail!(
                    "a {} manifest must not declare extension.generic",
                    manifest.plugin_type.as_str()
                );
            }
        }
        PluginType::Unknown(_) => unreachable!("unknown types are refused before this point"),
    }
    // Three worlds import `key-derivation`, and a manifest that asks for it anywhere else
    // would be granted an interface its world cannot name -- silent nonsense, which this
    // file refuses everywhere rather than ignores (RD-120-20).
    if manifest.capabilities.key_derivation
        && !matches!(
            manifest.plugin_type,
            PluginType::Auth | PluginType::Crawler | PluginType::StreamTransform
        )
    {
        bail!(
            "a {} manifest must not declare the key_derivation capability",
            manifest.plugin_type.as_str()
        );
    }
    // Computing over a credential without naming one is a grant that can never be used, and
    // the reference it computes over has to be one this plugin could already have sent.
    if manifest.capabilities.key_derivation && manifest.capabilities.secrets.is_empty() {
        bail!("key_derivation needs at least one capabilities.secrets reference");
    }
    // Only an `oauth` plugin has ways in to choose between; on any other type the field is
    // a mistake, and silently ignoring it would let an author believe it did something.
    if manifest.plugin_type != PluginType::OAuth && !manifest.oauth_flows.is_empty() {
        bail!(
            "a {} manifest must not declare oauth_flows",
            manifest.plugin_type.as_str()
        );
    }
    if !matches!(
        manifest.plugin_type,
        PluginType::Intake
            | PluginType::Auth
            | PluginType::OAuth
            | PluginType::Crawler
            | PluginType::Enricher
            | PluginType::Notifier
            | PluginType::Postprocess
            | PluginType::Storage
            | PluginType::RemoteJob
            | PluginType::StreamTransform
    ) && manifest.extension.is_some()
    {
        bail!(
            "a {} manifest must not declare [extension]",
            manifest.plugin_type.as_str()
        );
    }
    if let Some(stream) = &manifest.capabilities.net_stream {
        if stream.hosts.is_empty() || stream.ports.is_empty() {
            bail!("capabilities.net_stream needs at least one host and one port");
        }
        for host in &stream.hosts {
            validate_domain_pattern(host, false)?;
        }
        if stream.ports.contains(&0) {
            bail!("capabilities.net_stream port 0 is not a port");
        }
    }
    Ok(())
}

fn validate_capabilities(
    capabilities: &Capabilities,
    provider: Option<&ProviderManifest>,
) -> Result<()> {
    for reference in &capabilities.secrets {
        bounded_text("capabilities.secrets entry", reference, MAX_SLUG_CHARS * 2)?;
        if !reference
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            bail!("capabilities.secrets may only contain a-z, 0-9 and underscore");
        }
    }
    let Some(provider) = provider else {
        return Ok(());
    };
    // Every slot, not just the first: a provider that owns two references must be granted
    // both, or the second would be a credential slot nobody consented to.
    //
    // With one exception, and it is about which *plugin* holds the grant rather than about
    // consent (RD-106-03). An OAuth provider whose person registers their own application has
    // two slots, and they belong to two plugins: the sign-in expands the client secret, the
    // resolver expands the access token. A manifest carries one `plugin_type`, so they are
    // siblings and only one of them can own the provider row — and requiring that one to grant
    // itself the other's credential would be the opposite of least privilege. So the slot the
    // person fills may be declared without being granted here, and the sign-in plugin grants
    // it in its own manifest, the way `premiumize-crawler` grants a reference the resolver's
    // provider row owns.
    //
    // The same holds for a username-and-password provider with a flow slot (RD-120-30): the
    // sign-in plugin computes over the password, the file plugin uses the session, and neither
    // needs the other's credential.
    let sign_in_slot_is_a_siblings = matches!(
        provider.credentials,
        CredentialKindManifest::OAuth | CredentialKindManifest::UsernamePassword
    ) && provider
        .secret_slots()
        .iter()
        .any(|slot| slot.filled_by == SecretFilledByManifest::Flow);
    for slot in provider.secret_slots() {
        if sign_in_slot_is_a_siblings && slot.filled_by == SecretFilledByManifest::Person {
            continue;
        }
        if !capabilities.secrets.contains(&slot.reference) {
            bail!(
                "provider secret reference {} is not granted in capabilities.secrets",
                slot.reference
            );
        }
    }
    if provider.cookie_scope.is_some() && !capabilities.cookies {
        bail!("provider.cookie_scope requires the cookies capability");
    }
    Ok(())
}

fn validate_metadata(metadata: &PluginMetadata) -> Result<()> {
    bounded_text(
        "metadata.description",
        &metadata.description,
        MAX_DESCRIPTION_CHARS,
    )?;
    bounded_text("metadata.author", &metadata.author, MAX_AUTHOR_CHARS)?;
    if let Some(license) = &metadata.license {
        bounded_text("metadata.license", license, MAX_LICENSE_CHARS)?;
    }
    for (field, value) in [
        ("metadata.homepage", &metadata.homepage),
        ("metadata.support_url", &metadata.support_url),
    ] {
        let Some(value) = value else { continue };
        bounded_text(field, value, MAX_URL_CHARS)?;
        let url = url::Url::parse(value).with_context(|| format!("{field} is not a valid URL"))?;
        if url.scheme() != "https" {
            bail!("{field} must use https");
        }
    }
    if let Some(min_version) = &metadata.min_app_version {
        semver::Version::parse(min_version)
            .context("metadata.min_app_version is not semantic versioning")?;
    }
    Ok(())
}

fn validate_provider(provider: &ProviderManifest, domains: &[String]) -> Result<()> {
    validate_slug(&provider.slug)?;
    // A provider that takes no account must not describe one. Nothing would ever fill these:
    // the accounts list hides such a provider, so a declared secret or cookie scope would be a
    // credential no one can enter and a grant no one asked for (RD-098-01).
    if provider.credentials == CredentialKindManifest::NoneRequired {
        if provider.secret_reference.is_some() {
            bail!("provider.credentials = \"none\" cannot declare a secret_reference");
        }
        if !provider.secret_domains.is_empty() {
            bail!("provider.credentials = \"none\" cannot declare secret_domains");
        }
        if !provider.secrets.is_empty() {
            bail!("provider.credentials = \"none\" cannot declare secrets");
        }
        if provider.cookie_scope.is_some() {
            bail!("provider.credentials = \"none\" cannot declare a cookie_scope");
        }
        if provider.username_required {
            bail!("provider.credentials = \"none\" cannot require a username");
        }
    }
    // One spelling or the other, never both: a manifest that sets each of them would leave
    // which slots actually exist up to the reader.
    if !provider.secrets.is_empty()
        && (provider.secret_reference.is_some() || !provider.secret_domains.is_empty())
    {
        bail!(
            "provider.secrets cannot be combined with provider.secret_reference or provider.secret_domains"
        );
    }
    if !provider.secret_domains.is_empty() && provider.secret_reference.is_none() {
        bail!("provider.secret_domains requires provider.secret_reference");
    }
    let slots = provider.secret_slots();
    for (index, slot) in slots.iter().enumerate() {
        // Ownership of a reference is enforced when the row is registered
        // (`rd_provider_registry` refuses one another provider already claims); here we only
        // check it is a plain identifier that can appear in a `{{secret:…}}` marker.
        bounded_text(
            "provider secret reference",
            &slot.reference,
            MAX_SLUG_CHARS * 2,
        )?;
        if !slot
            .reference
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            bail!("provider secret reference may only contain a-z, 0-9 and underscore");
        }
        if slots[..index]
            .iter()
            .any(|earlier| earlier.reference == slot.reference)
        {
            bail!(
                "provider declares the secret reference {} twice",
                slot.reference
            );
        }
        for host in &slot.domains {
            validate_domain_pattern(host, false)?;
            if !host_covered(host, domains) {
                bail!("provider secret domain {host} is outside the plugin's domains");
            }
        }
    }
    // HTTP Basic on the transfer is built from exactly one credential the person typed, and it
    // goes to exactly the hosts that credential's slot names (RD-120-38). So it needs one slot,
    // with hosts, of a kind whose secret *is* that credential: not a token a flow obtained, not
    // a cookie jar, not one of two modes the engine would have to choose between.
    if provider.transfer_auth == TransferAuthManifest::Basic {
        if !matches!(
            provider.credentials,
            CredentialKindManifest::ApiKey | CredentialKindManifest::UsernamePassword
        ) {
            bail!(
                "provider.transfer_auth = \"basic\" requires credentials = \"api_key\" or \"username_password\""
            );
        }
        match slots.as_slice() {
            [slot] if !slot.domains.is_empty() => {}
            _ => bail!(
                "provider.transfer_auth = \"basic\" requires one secret_reference with secret_domains"
            ),
        }
    }
    // A choice of modes is only meaningful if every slot says which mode it serves and both
    // modes are actually described; anything else would leave an account unable to reach a
    // credential it was allowed to enter.
    if provider.credentials == CredentialKindManifest::LoginOrApiKey {
        if slots.iter().any(|slot| slot.mode.is_none()) {
            bail!(
                "provider.credentials = \"login_or_api_key\" requires a mode on every provider.secrets entry"
            );
        }
        for required in [
            CredentialModeManifest::Login,
            CredentialModeManifest::ApiKey,
        ] {
            if !slots.iter().any(|slot| slot.mode == Some(required)) {
                bail!(
                    "provider.credentials = \"login_or_api_key\" needs a provider.secrets entry for each mode"
                );
            }
        }
    } else if slots.iter().any(|slot| slot.mode.is_some()) {
        bail!("provider.secrets may only declare a mode when credentials = \"login_or_api_key\"");
    }
    // A slot the flow fills only makes sense where a flow fills one, and only beside a slot
    // the person fills -- otherwise the account would have a credential nobody can enter, or
    // a sign-in with nowhere to put what it obtained (RD-106-03).
    //
    // Two kinds of provider have such a flow: an OAuth one whose person registers their own
    // application, and -- since RD-120-30 -- a username-and-password one whose sign-in plugin
    // turns the password into a session. MEGA is the second: the password has to survive the
    // sign-in, or the next one has nothing to start from.
    let flow_slots = slots
        .iter()
        .filter(|slot| slot.filled_by == SecretFilledByManifest::Flow)
        .count();
    if flow_slots > 0 {
        if !matches!(
            provider.credentials,
            CredentialKindManifest::OAuth | CredentialKindManifest::UsernamePassword
        ) {
            bail!(
                "provider.secrets may only declare filled_by = \"flow\" when credentials = \"oauth\" or \"username_password\""
            );
        }
        if flow_slots > 1 {
            bail!("provider.secrets may declare at most one filled_by = \"flow\" entry");
        }
        if slots.len() != 2 {
            bail!(
                "a provider with a filled_by = \"flow\" slot needs exactly one other slot, for what the person enters"
            );
        }
    }
    if let Some(scope) = &provider.cookie_scope {
        let url = url::Url::parse(scope).context("provider.cookie_scope is not a valid URL")?;
        if url.scheme() != "https" {
            bail!("provider.cookie_scope must use https");
        }
        let host = url
            .host_str()
            .context("provider.cookie_scope needs a host")?
            .to_ascii_lowercase();
        if !host_covered(&host, domains) {
            bail!("provider.cookie_scope host {host} is outside the plugin's domains");
        }
    }
    for (alias, canonical) in &provider.host_aliases {
        validate_domain_pattern(alias, false)?;
        validate_domain_pattern(canonical, false)?;
    }
    if provider.kind == ProviderKindManifest::Multihoster && !provider.host_aliases.is_empty() {
        bail!("multihoster providers cannot declare host_aliases");
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.len() < MIN_SLUG_CHARS || slug.len() > MAX_SLUG_CHARS {
        bail!("provider.slug must be {MIN_SLUG_CHARS}-{MAX_SLUG_CHARS} characters");
    }
    if !slug
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("provider.slug may only contain a-z, 0-9 and underscore");
    }
    Ok(())
}

fn bounded_text(field: &str, value: &str, limit: usize) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} is required");
    }
    if value.chars().count() > limit {
        bail!("{field} exceeds {limit} characters");
    }
    Ok(())
}

/// Whether `host` is served by one of the plugin's sandbox `domains`.
///
/// `*.suffix` covers sub-domains only, never the bare suffix — the same reading
/// [`crate::domain_allowed`] applies at request time. Two wildcard semantics for one
/// allowlist is how a manifest ends up promising less than the runtime permits.
fn host_covered(host: &str, domains: &[String]) -> bool {
    domains.iter().any(|domain| {
        domain == host
            || domain
                .strip_prefix("*.")
                .is_some_and(|suffix| host.ends_with(&format!(".{suffix}")))
    })
}

pub(crate) fn validate_domain_pattern(domain: &str, allow_all: bool) -> Result<()> {
    if domain == "*" {
        // `*` passed every check below — not uppercase, no `/`, no `:`, and non-empty once
        // dots are trimmed — so `allow_all = false` enforced nothing and a sandbox allowlist
        // of `["*"]` reached every http and https host there is.
        return if allow_all {
            Ok(())
        } else {
            bail!("plugin domain {domain} may not be the catch-all wildcard here")
        };
    }
    if domain != domain.to_ascii_lowercase()
        || domain.contains('/')
        || domain.contains(':')
        || domain.trim_matches('.').is_empty()
    {
        bail!("invalid plugin domain {domain}");
    }
    Ok(())
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
mod tests {
    use super::*;

    /// A manifest for one of the six extension types.
    fn extension_toml(plugin_type: &str, extra: &str) -> String {
        format!(
            r#"manifest_version = 3
plugin_type = "{plugin_type}"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-00000000abce"
name = "Fixture Extension"
version = "0.1.0"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A fixture extension"
author = "Fixture Author"

[extension]
slug = "fixture_extension"
{extra}
"#
        )
    }

    /// A manifest of `plugin_type`, with `extra` written above the first table so a
    /// top-level key stays a top-level key.
    ///
    /// The trap this exists to avoid is TOML's, not this parser's: a bare key written after
    /// `[capabilities.net_http]` belongs to that table, so `oauth_flows` placed there would
    /// silently become an unknown capability rather than a list of ways in.
    fn toplevel_toml(plugin_type: &str, extra: &str) -> String {
        format!(
            r#"manifest_version = 3
plugin_type = "{plugin_type}"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-00000000abcf"
name = "Fixture Extension"
version = "0.1.0"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
{extra}

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A fixture extension"
author = "Fixture Author"

[extension]
slug = "fixture_extension"
"#
        )
    }

    /// An OAuth manifest written before RD-106-01 still means what it meant: the redirect.
    ///
    /// The compatibility promise of the whole job sits in this default. If a silent manifest
    /// were read as offering both, the host would call a device entrance nobody implemented
    /// the first time somebody signed in.
    #[test]
    fn an_oauth_manifest_without_the_field_offers_the_redirect() {
        let manifest: PluginManifest = toml::from_str(&toplevel_toml("oauth", "")).expect("parses");
        validate_manifest(&manifest).expect("valid");
        assert_eq!(
            manifest.oauth_flows(),
            [OAuthFlowManifest::Redirect].as_slice()
        );
        assert!(manifest.serves_oauth_flow(OAuthFlowManifest::Redirect));
        assert!(!manifest.serves_oauth_flow(OAuthFlowManifest::Device));
    }

    #[test]
    fn an_oauth_manifest_may_offer_both_ways_in_and_states_their_order() {
        let manifest: PluginManifest = toml::from_str(&toplevel_toml(
            "oauth",
            r#"oauth_flows = ["device", "redirect"]"#,
        ))
        .expect("parses");
        validate_manifest(&manifest).expect("valid");
        assert_eq!(
            manifest.oauth_flows(),
            [OAuthFlowManifest::Device, OAuthFlowManifest::Redirect].as_slice()
        );
    }

    /// Two refusals, both of them a mistake an author would otherwise never hear about: the
    /// field on a type that has no ways in, and one entrance named twice.
    #[test]
    fn oauth_flows_is_refused_where_it_means_nothing() {
        let wrong_type: PluginManifest =
            toml::from_str(&toplevel_toml("auth", r#"oauth_flows = ["device"]"#)).expect("parses");
        let error = validate_manifest(&wrong_type).expect_err("refused");
        assert!(error.to_string().contains("oauth_flows"), "{error}");

        let repeated: PluginManifest = toml::from_str(&toplevel_toml(
            "oauth",
            r#"oauth_flows = ["device", "device"]"#,
        ))
        .expect("parses");
        let error = validate_manifest(&repeated).expect_err("refused");
        assert!(error.to_string().contains("twice"), "{error}");
    }

    /// The declaration that puts a link fragment in the vault (RD-110-38).
    ///
    /// Written above the first table on purpose: a bare key after `[capabilities.net_http]`
    /// belongs to that table, and this one silently became an unknown capability the first
    /// time it was written there. The catch-all is refused, because `*` would turn every
    /// fragment on the internet into key material.
    #[test]
    fn a_secret_fragment_declaration_names_hosts_and_never_everything() {
        let declared: PluginManifest = toml::from_str(&toplevel_toml(
            "stream-transform",
            r#"secret_fragment_domains = ["example.test", "*.example.test"]"#,
        ))
        .expect("parses");
        validate_manifest(&declared).expect("valid");
        assert_eq!(
            declared.secret_fragment_domains,
            ["example.test".to_owned(), "*.example.test".to_owned()]
        );

        let everything: PluginManifest = toml::from_str(&toplevel_toml(
            "stream-transform",
            r#"secret_fragment_domains = ["*"]"#,
        ))
        .expect("parses");
        validate_manifest(&everything).expect_err("the catch-all is refused");

        // Absent is the ordinary case, and it is what every manifest written so far means.
        let silent: PluginManifest =
            toml::from_str(&toplevel_toml("stream-transform", "")).expect("parses");
        validate_manifest(&silent).expect("valid");
        assert!(silent.secret_fragment_domains.is_empty());
    }

    /// A notification destination may say "wherever the destination points" (RD-130-15),
    /// because the host narrows it to that one host per delivery. A type whose allowlist is
    /// its boundary still may not.
    #[test]
    fn a_notifier_may_declare_the_catch_all_and_an_enricher_may_not() {
        let with_wildcard = |plugin_type: &str| -> PluginManifest {
            toml::from_str(&toplevel_toml(plugin_type, "").replace(
                r#"domains = ["example.test"]"#,
                r#"domains = ["example.test", "*"]"#,
            ))
            .expect("parses")
        };
        validate_manifest(&with_wildcard("notifier")).expect("a notifier narrows per delivery");
        let error = validate_manifest(&with_wildcard("enricher")).expect_err("refused");
        assert!(error.to_string().contains("catch-all"), "{error}");
    }

    #[test]
    fn every_extension_type_is_accepted_with_its_section() {
        for plugin_type in [
            "intake",
            "auth",
            "enricher",
            "notifier",
            "postprocess",
            "storage",
        ] {
            let manifest: PluginManifest =
                toml::from_str(&extension_toml(plugin_type, "")).expect(plugin_type);
            assert_eq!(manifest.plugin_type.as_str(), plugin_type);
            assert_eq!(manifest.message_slug(), "fixture_extension");
            validate_manifest(&manifest).unwrap_or_else(|error| panic!("{plugin_type}: {error}"));
        }
    }

    #[test]
    fn an_extension_manifest_without_its_section_is_refused() {
        // Without a slug there is no namespace for the plugin's messages, and the
        // alternative — inventing one — makes two plugins collide silently.
        let raw = extension_toml("notifier", "")
            .replace("[extension]\nslug = \"fixture_extension\"\n", "");
        let manifest: PluginManifest = toml::from_str(&raw).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn an_extension_manifest_may_not_claim_a_provider_or_a_transfer() {
        // Each section grants a different thing. A manifest declaring two of them is either
        // confused or trying for both, and neither is something to resolve by guessing.
        for extra in [
            "\n[provider]\nslug = \"x\"\nkind = \"hoster\"\ncredentials = \"api_key\"\n",
            "\n[transfer]\nslug = \"x\"\nschemes = [\"x\"]\n",
        ] {
            let manifest: PluginManifest =
                toml::from_str(&extension_toml("intake", extra)).expect("parse");
            assert!(validate_manifest(&manifest).is_err(), "{extra}");
        }
    }

    #[test]
    fn a_resolver_may_not_declare_an_extension_section() {
        let manifest: PluginManifest =
            toml::from_str(&manifest_toml("\n[extension]\nslug = \"sneaky\"\n")).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn a_storage_destination_must_ask_for_a_way_out() {
        // A destination with no outbound grant could not upload anything; such a manifest is
        // a mistake, not a plugin that stores to nowhere.
        let raw = extension_toml("storage", "").replace(
            "[capabilities.net_http]\ndomains = [\"example.test\"]\n",
            "",
        );
        let manifest: PluginManifest = toml::from_str(&raw).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    fn manifest_toml(extra: &str) -> String {
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-00000000abcd"
name = "Fixture"
version = "1.2.3"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities]
cookies = true
captcha = true
secrets = ["fixture_api_key", "onefichier_api_key"]

[capabilities.net_http]
domains = ["example.test", "*.example.test"]

[metadata]
description = "A fixture resolver"
author = "Fixture Author"

[provider]
slug = "fixture"
kind = "hoster"
credentials = "api_key"
{extra}
"#
        )
    }

    fn parse(extra: &str) -> Result<PluginManifest> {
        let manifest: PluginManifest = toml::from_str(&manifest_toml(extra))?;
        validate_manifest(&manifest)?;
        Ok(manifest)
    }

    #[test]
    fn minimal_v3_manifest_validates() {
        let manifest = parse("").expect("valid manifest");
        assert_eq!(manifest.message_slug(), "fixture");
        assert_eq!(manifest.metadata.author, "Fixture Author");
        assert!(manifest.verifying_key().is_ok());
    }

    #[test]
    fn requires_account_defaults_to_true_and_can_be_disabled() {
        let manifest = parse("").expect("valid manifest");
        assert!(manifest.requires_account);
        let toml = manifest_toml("").replace(
            "max_concurrent_downloads = 1",
            "max_concurrent_downloads = 1\nrequires_account = false",
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        validate_manifest(&manifest).expect("valid manifest");
        assert!(!manifest.requires_account);
    }

    #[test]
    fn an_older_manifest_revision_is_refused_as_outdated() {
        let toml = manifest_toml("").replace("manifest_version = 3", "manifest_version = 2");
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        let error = validate_manifest(&manifest).expect_err("v2 is no longer accepted");
        let rejection = error
            .downcast_ref::<ManifestRejection>()
            .expect("typed rejection");
        assert_eq!(rejection.code(), "plugin.manifest_outdated");
    }

    #[test]
    fn an_unknown_plugin_type_is_refused_rather_than_treated_as_a_resolver() {
        // Deliberately a name no future plugin type will take. The original example here
        // was "notifier", which stopped being unknown the moment that type was added — the
        // test then asserted the opposite of what it was written to check.
        let toml = manifest_toml("").replace(
            r#"plugin_type = "resolver""#,
            r#"plugin_type = "teleporter""#,
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        assert_eq!(
            manifest.plugin_type,
            PluginType::Unknown("teleporter".into())
        );
        let error = validate_manifest(&manifest).expect_err("unknown type");
        assert_eq!(
            error
                .downcast_ref::<ManifestRejection>()
                .expect("typed rejection")
                .code(),
            "plugin.capability_unknown"
        );
    }

    #[test]
    fn key_derivation_is_only_for_the_types_whose_world_imports_it() {
        // Three worlds import `key-derivation` (RD-120-20). On any other type the grant
        // would be an interface the world cannot name, which is silent nonsense.
        for plugin_type in ["auth", "crawler", "stream-transform"] {
            let toml = extension_toml(
                plugin_type,
                "",
            )
            .replace(
                "[capabilities.net_http]",
                "[capabilities]\nkey_derivation = true\nsecrets = [\"fixture_password\"]\n\n[capabilities.net_http]",
            );
            let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
            validate_manifest(&manifest)
                .unwrap_or_else(|error| panic!("{plugin_type} should be allowed: {error}"));
        }
        let toml = extension_toml("notifier", "").replace(
            "[capabilities.net_http]",
            "[capabilities]\nkey_derivation = true\nsecrets = [\"fixture_password\"]\n\n[capabilities.net_http]",
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        let error = validate_manifest(&manifest).expect_err("a notifier must not declare it");
        assert!(
            format!("{error}").contains("key_derivation"),
            "the refusal does not name the grant: {error}"
        );
    }

    #[test]
    fn key_derivation_without_a_named_secret_is_refused() {
        // Computing over a credential without naming one is a grant that can never be used.
        let toml = extension_toml("auth", "").replace(
            "[capabilities.net_http]",
            "[capabilities]\nkey_derivation = true\n\n[capabilities.net_http]",
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        let error = validate_manifest(&manifest).expect_err("a grant with nothing to reach");
        assert!(
            format!("{error}").contains("capabilities.secrets"),
            "{error}"
        );
        assert!(
            manifest
                .capabilities
                .granted()
                .contains(&"key_derivation".to_owned())
        );
    }

    #[test]
    fn a_stream_transform_plugin_may_own_the_provider_it_claims() {
        // The one exception to "an extension type declares no [provider]" (RD-120-20): such
        // a plugin is a resolver in everything but the world it exports, and MEGA had no
        // provider row at all until it declared one.
        let provider = r#"
[provider]
slug = "fixture_provider"
kind = "hoster"
credentials = "username_password"
username_required = true
secret_reference = "fixture_password"
secret_domains = ["example.test"]
"#;
        let with_secret = |plugin_type: &str| {
            format!(
                "{}{provider}",
                extension_toml(plugin_type, "").replace(
                    "[capabilities.net_http]",
                    "[capabilities]\nsecrets = [\"fixture_password\"]\n\n[capabilities.net_http]",
                )
            )
        };
        let manifest: PluginManifest =
            toml::from_str(&with_secret("stream-transform")).expect("parse");
        validate_manifest(&manifest).expect("a stream-transform plugin may own its provider");
        // Every other extension type still may not.
        let manifest: PluginManifest = toml::from_str(&with_secret("crawler")).expect("parse");
        let error = validate_manifest(&manifest).expect_err("a crawler must not");
        assert!(format!("{error}").contains("[provider]"), "{error}");
    }

    #[test]
    fn a_password_provider_may_keep_its_sign_in_s_session_in_a_slot_of_its_own() {
        // RD-120-30: MEGA's password has to survive its sign-in, so the session gets a flow
        // slot the way an OAuth token beside a registered application does. Any other kind of
        // credential still may not declare one.
        let slots = |credentials: &str| {
            [
                extension_toml("stream-transform", "").replace(
                    "[capabilities.net_http]",
                    "[capabilities]\nsecrets = [\"fixture_password\", \"fixture_session\"]\n\n[capabilities.net_http]",
                ),
                format!(
                    r#"
[provider]
slug = "fixture_provider"
kind = "hoster"
credentials = "{credentials}"

[[provider.secrets]]
reference = "fixture_password"
domains = ["example.test"]

[[provider.secrets]]
reference = "fixture_session"
domains = ["example.test"]
filled_by = "flow"
"#
                ),
            ]
            .concat()
        };
        let manifest: PluginManifest = toml::from_str(&slots("username_password")).expect("parse");
        validate_manifest(&manifest).expect("a password provider may keep a session beside it");
        let manifest: PluginManifest = toml::from_str(&slots("api_key")).expect("parse");
        let error = validate_manifest(&manifest).expect_err("an API key has no sign-in");
        assert!(format!("{error}").contains("filled_by"), "{error}");
    }

    #[test]
    fn an_unknown_capability_is_refused_rather_than_ignored() {
        // A capability that reads like something a plugin might plausibly want, and that this
        // build has no interface for — exactly the case that must not be silently ignored.
        let toml = manifest_toml("").replace("cookies = true", "cookies = true\nfilesystem = true");
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        let error = validate_manifest(&manifest).expect_err("unknown capability");
        assert_eq!(
            error
                .downcast_ref::<ManifestRejection>()
                .expect("typed rejection")
                .code(),
            "plugin.capability_unknown"
        );
    }

    #[test]
    fn a_manifest_spells_the_oauth_credential_kind_the_way_the_documentation_does() {
        // `rename_all = "snake_case"` would have made this `o_auth`. The documentation says
        // `oauth`, and a manifest author has only the documentation to go on.
        let toml =
            manifest_toml("").replace(r#"credentials = "api_key""#, r#"credentials = "oauth""#);
        assert!(
            toml.contains(r#"credentials = "oauth""#),
            "the fixture no longer spells its credential kind the way this test expects"
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("manifest parses");
        assert_eq!(
            manifest.provider.expect("provider").credentials,
            CredentialKindManifest::OAuth
        );
    }

    #[test]
    fn an_unsupported_api_version_is_refused() {
        let toml =
            manifest_toml("").replace(r#"api_version = "0.9.0""#, r#"api_version = "0.5.0""#);
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    /// RD-130-11: a package built for `0.8.0` is refused, and under a code the plugin manager
    /// names, rather than failing at the linker. The release note quotes this code. (RD-120-36
    /// asked the same of `0.7.0`; the contract moved again for `cache-kinds`/`check-cached`.)
    #[test]
    fn a_package_built_for_the_previous_contract_is_refused_by_name() {
        let toml =
            manifest_toml("").replace(r#"api_version = "0.9.0""#, r#"api_version = "0.8.0""#);
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        let error = validate_manifest(&manifest).expect_err("0.8.0 no longer links");
        let rejection = error
            .downcast_ref::<ManifestRejection>()
            .expect("a rejection the plugin manager can name");
        assert_eq!(rejection.code(), "plugin.capability_unknown");
    }

    #[test]
    fn a_cookie_scope_without_the_cookies_grant_is_refused() {
        let toml = manifest_toml(r#"cookie_scope = "https://example.test/""#)
            .replace("cookies = true\n", "");
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn secret_reference_must_be_granted_and_a_plain_identifier() {
        assert!(parse(r#"secret_reference = "fixture_api_key""#).is_ok());
        // A provider whose slug is not a valid identifier prefix (like `1fichier`) may still
        // name its reference freely; ownership is enforced at registration.
        assert!(parse(r#"secret_reference = "onefichier_api_key""#).is_ok());
        // Declared but never granted: the manifest would promise a credential the runtime
        // does not hand over, and the plugin manager would show a grant nobody enforces.
        assert!(parse(r#"secret_reference = "fixture_other_key""#).is_err());
        assert!(parse(r#"secret_reference = "Fixture-Key""#).is_err());
    }

    #[test]
    fn a_wildcard_domain_does_not_cover_its_bare_suffix() {
        // The same reading `domain_allowed` applies at request time: `*.example.test`
        // grants sub-domains, not `example.test` itself.
        assert!(host_covered(
            "api.example.test",
            &["*.example.test".to_owned()]
        ));
        assert!(!host_covered(
            "example.test",
            &["*.example.test".to_owned()]
        ));
        assert!(host_covered("example.test", &["example.test".to_owned()]));
    }

    #[test]
    fn secret_domains_must_stay_inside_the_sandbox() {
        assert!(
            parse(
                r#"secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#
            )
            .is_ok()
        );
        assert!(
            parse(
                r#"secret_reference = "fixture_api_key"
secret_domains = ["evil.invalid"]"#
            )
            .is_err()
        );
    }

    /// `transfer_auth = "basic"` hands the engine one credential and the hosts it may reach
    /// (RD-120-38), so a manifest that cannot name both is refused rather than guessed at.
    #[test]
    fn transfer_auth_basic_needs_one_secret_with_hosts() {
        let row = |extra: &str| {
            provider_spec_from_manifest(&parse(extra).expect("valid manifest"))
                .expect("a provider row")
                .spec
                .transfer_auth
        };
        assert_eq!(
            row(r#"secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#),
            rd_provider_registry::TransferAuth::None,
            "absent means the transfer carries nothing"
        );
        assert_eq!(
            row(r#"transfer_auth = "basic"
secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#),
            rd_provider_registry::TransferAuth::Basic
        );
        // No hosts, no secret, or a kind whose secret is not the credential: refused.
        for extra in [
            r#"transfer_auth = "basic"
secret_reference = "fixture_api_key""#,
            r#"transfer_auth = "basic""#,
        ] {
            assert!(parse(extra).is_err(), "{extra}");
        }
        let cookies = manifest_toml(
            r#"transfer_auth = "basic"
secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#,
        )
        .replace(
            r#"credentials = "api_key""#,
            r#"credentials = "api_key_or_cookies""#,
        );
        let manifest: PluginManifest = toml::from_str(&cookies).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn cookie_scope_must_be_https_and_inside_the_sandbox() {
        assert!(parse(r#"cookie_scope = "https://example.test/""#).is_ok());
        assert!(parse(r#"cookie_scope = "http://example.test/""#).is_err());
        assert!(parse(r#"cookie_scope = "https://evil.invalid/""#).is_err());
    }

    #[test]
    fn slug_charset_is_enforced() {
        for slug in ["Fixture", "fix-ture", "f", "fix ture"] {
            let toml =
                manifest_toml("").replace(r#"slug = "fixture""#, &format!(r#"slug = "{slug}""#));
            let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
            assert!(
                validate_manifest(&manifest).is_err(),
                "{slug} should be rejected"
            );
        }
    }

    #[test]
    fn metadata_urls_must_be_https() {
        let toml = manifest_toml("").replace(
            r#"author = "Fixture Author""#,
            r#"author = "Fixture Author"
homepage = "http://example.test""#,
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn min_app_version_gates_installation() {
        let toml = manifest_toml("").replace(
            r#"author = "Fixture Author""#,
            r#"author = "Fixture Author"
min_app_version = "0.9.0""#,
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        validate_manifest(&manifest).expect("valid");
        assert!(check_app_version(&manifest, "0.8.0").is_err());
        assert!(check_app_version(&manifest, "0.9.0").is_ok());
        assert!(check_app_version(&manifest, "1.0.0").is_ok());
    }

    #[test]
    fn derived_spec_uses_the_manifest_as_the_only_authority() {
        let toml = manifest_toml(
            r#"secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]
cookie_scope = "https://example.test/""#,
        )
        .replace(
            "max_concurrent_downloads = 1",
            r#"match_domains = ["example.test"]
max_concurrent_downloads = 1"#,
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        validate_manifest(&manifest).expect("valid");

        let row = provider_spec_from_manifest(&manifest).expect("resolver contributes a row");
        assert_eq!(row.plugin_id, manifest.id.to_string());
        assert_eq!(row.spec.slug, "fixture");
        assert_eq!(row.spec.display_name, "Fixture");
        assert_eq!(
            row.spec.source,
            rd_provider_registry::ProviderSource::Plugin
        );
        // The sandbox allowlist is what the provider may talk to.
        assert_eq!(row.spec.request_domains, manifest.domains());
        assert_eq!(row.spec.secret_reference(), Some("fixture_api_key"));
        assert_eq!(row.spec.match_hosts, vec!["example.test".to_owned()]);
    }

    #[test]
    fn a_multihoster_never_claims_urls() {
        let toml = manifest_toml("")
            .replace(r#"kind = "hoster""#, r#"kind = "multihoster""#)
            .replace(
                "max_concurrent_downloads = 1",
                r#"match_domains = ["*"]
max_concurrent_downloads = 1"#,
            );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        validate_manifest(&manifest).expect("valid");
        assert!(
            provider_spec_from_manifest(&manifest)
                .expect("resolver contributes a row")
                .spec
                .match_hosts
                .is_empty()
        );
    }

    #[test]
    fn match_hosts_drops_wildcard_intake_patterns() {
        let toml = manifest_toml("").replace(
            "max_concurrent_downloads = 1",
            r#"match_domains = ["example.test", "*", "*.example.test"]
max_concurrent_downloads = 1"#,
        );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        validate_manifest(&manifest).expect("valid");
        assert_eq!(manifest.match_hosts(), vec!["example.test".to_owned()]);
    }

    /// The resolver fixture with a provider that takes no account at all (RD-098-01).
    fn account_less_toml(extra: &str) -> String {
        manifest_toml(extra).replace(r#"credentials = "api_key""#, r#"credentials = "none""#)
    }

    #[test]
    fn a_provider_that_takes_no_account_reaches_the_registry() {
        let manifest: PluginManifest = toml::from_str(&account_less_toml("")).expect("parse");
        validate_manifest(&manifest).expect("valid");
        assert_eq!(
            manifest.provider.as_ref().expect("provider").credentials,
            CredentialKindManifest::NoneRequired
        );

        let row = provider_spec_from_manifest(&manifest).expect("resolver contributes a row");
        assert_eq!(
            row.spec.credentials,
            rd_provider_registry::CredentialKind::NoneRequired
        );
        assert!(row.spec.secret_reference().is_none());
        assert!(row.spec.cookie_scope.is_none());
    }

    #[test]
    fn a_provider_that_takes_no_account_may_not_describe_one() {
        // Each of these would be a credential nobody can enter: the accounts list leaves such a
        // provider out, so the field would sit in the manifest granting reach for nothing.
        for extra in [
            "secret_reference = \"fixture_api_key\"",
            "cookie_scope = \"https://example.test/\"",
            "username_required = true",
        ] {
            let manifest: PluginManifest =
                toml::from_str(&account_less_toml(extra)).expect("parse");
            assert!(validate_manifest(&manifest).is_err(), "{extra}");
        }
    }
}

//! The sections of a manifest and the types they are written in: `[capabilities]`,
//! `[metadata]`, `[transfer]`, `[extension]` and `[provider]` with its credential slots.
//!
//! Split out of `manifest.rs` (PLUG-21); what the sections may contain is checked in
//! `validate.rs` and `checks.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    /// What a notification target of this destination may be set to (RD-170-09), each a name
    /// and the values it accepts. Only a notifier declares any: the host checks a target's
    /// settings against this list when it is saved and answers the plugin's
    /// `destination-settings.setting` from it at delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<SettingManifest>,
    /// The container formats a `remote-job` plugin's provider takes as `job-source::container`
    /// (RD-191-13): any of [`REMOTE_JOB_CONTAINERS`]. Declared rather than probed, so the
    /// LinkGrabber can offer an NZB only to the accounts whose provider takes one without
    /// compiling a component to ask. Empty: the plugin takes no container, or says nothing --
    /// either way nothing is offered on its behalf, and `identify` still has the last word.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub containers: Vec<String>,
}

/// What `[extension] containers` may name.
pub const REMOTE_JOB_CONTAINERS: &[&str] = &["torrent", "nzb", "dlc", "rsdf"];

/// One setting a notification destination offers (`[[extension.settings]]`, RD-170-09).
///
/// A choice among fixed values rather than free text: what a person can pick is what the
/// plugin was written to understand, and a value outside the list is refused when the target
/// is saved instead of being guessed at on every delivery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SettingManifest {
    /// The key a target stores and the plugin asks for; lowercase, digits and underscores.
    /// Its label is the plugin's own code `<slug>.setting.<name>`.
    pub name: String,
    /// Every value the setting accepts, in the order the interface offers them. A value's
    /// label is the plugin's code `<slug>.choice.<value>`, or the value itself.
    pub choices: Vec<String>,
    /// What the plugin is told when a target leaves the setting alone. Absent means the
    /// setting may stay unset, and the plugin is told `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
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
    /// Sign in with a code, or paste a ready-made API key, chosen per account (RD-150-09).
    /// Written `credentials = "oauth_or_api_key"`. Every `[[provider.secrets]]` entry names a
    /// mode: the `api_key` one is the slot the person fills, the `oauth` ones are filled by the
    /// sign-in -- the access token first, then the named parts it keeps beside the token.
    #[serde(rename = "oauth_or_api_key")]
    OAuthOrApiKey,
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
    /// Signed in with a code; only on a `credentials = "oauth_or_api_key"` provider.
    #[serde(rename = "oauth")]
    OAuth,
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

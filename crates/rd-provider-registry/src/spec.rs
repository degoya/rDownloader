//! The provider row and the types it is written in: what kind of provider it is, the
//! credentials an account stores and how they travel, its secret slots, and the row a plugin
//! contributes together with the error a refused one reports.
//!
//! Split out of `lib.rs` (PLUG-21); the table itself and its lookups stay there.

/// Whether a provider resolves links for its own domains (`Hoster`) or resolves links for
/// other hosters' domains on behalf of the account (`Multihoster`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Hoster,
    Multihoster,
}

/// The shape of the credential(s) a provider account stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    ApiKey,
    UsernamePassword,
    ApiKeyOrCookies,
    Cookies,
    /// Two mutually exclusive ways to hold the same account, picked per account: sign in with
    /// username and password, or paste a ready-made API key. The account's `credential_mode`
    /// says which one is active, and that in turn decides which of the provider's
    /// [`SecretSlot`]s the resolver may use.
    LoginOrApiKey,
    /// The account is signed in through a redirect and kept alive by renewal (RD-103-00).
    ///
    /// What the person supplies is their own OAuth client, not a credential: the project ships
    /// no client id of its own. Everything after that -- the code, the token, the refresh
    /// material -- is obtained and stored by the flow, so the accounts form asks for a sign-in
    /// rather than for something to paste.
    OAuth,
    /// Two ways to hold one account, picked per account (RD-150-09): sign in with a code the
    /// person confirms at the provider, or paste a ready-made API key. The account's
    /// `credential_mode` says which ([`CredentialMode::OAuth`] or [`CredentialMode::ApiKey`]).
    /// In the first mode every slot is filled by the flow -- the access token, and the named
    /// parts a sign-in keeps beside it -- and nothing is typed; in the second the one slot the
    /// person fills is the account's credential, exactly as for [`Self::ApiKey`].
    OAuthOrApiKey,
    /// No account at all. The provider registers for resolving and is hidden from the accounts
    /// list, because there is nothing to enter (RD-098-01).
    NoneRequired,
}

/// How the account's own credential travels on a **transfer**, as opposed to on a resolver's
/// API calls (RD-120-38).
///
/// A resolver reaches its provider through `{{secret:…}}` and `{{basic:…}}` markers the host
/// expands, but the bytes are fetched by the download engine, which never runs plugin code. So
/// a provider whose file addresses themselves want the account's credential has to say so on
/// its row, and the engine attaches it -- only over TLS and only to the hosts that credential's
/// slot names, checked against the address the transfer actually goes to.
///
/// An OAuth provider needs no declaration: its token is always a `Bearer` credential and has
/// been attached that way since RD-106-04. This enum is for the other shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransferAuth {
    /// The transfer carries nothing the account holds. Every provider row written before
    /// RD-120-38 means this.
    #[default]
    None,
    /// `Authorization: Basic base64(username:secret)`. The user name may be empty only when the
    /// row does not require one ([`ProviderSpec::username_required`]), which is how Pixeldrain
    /// takes an API key; for every other provider an empty name is half a credential.
    Basic,
}

/// Which of a provider's credential modes an account uses.
///
/// Only meaningful for [`CredentialKind::LoginOrApiKey`] and [`CredentialKind::OAuthOrApiKey`];
/// every other kind has exactly one way to hold an account and stores `None`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// The single secret slot holds an account password, and the resolver signs in with it.
    Login,
    /// The single secret slot holds a ready-made API key.
    ApiKey,
    /// Nothing is typed: a sign-in with a code fills the account's slots (RD-150-09).
    ///
    /// Renamed explicitly, because `rename_all = "snake_case"` would spell it `o_auth`.
    #[serde(rename = "oauth")]
    OAuth,
}

impl CredentialMode {
    /// The wire spelling, as stored in `accounts.credential_mode` and sent over REST.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
        }
    }

    /// Parses the wire spelling; unknown values are rejected rather than silently defaulted,
    /// so a typo in a restored backup cannot quietly change which host a password reaches.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "login" => Some(Self::Login),
            "api_key" => Some(Self::ApiKey),
            "oauth" => Some(Self::OAuth),
            _ => None,
        }
    }
}

/// One named credential slot a provider owns.
///
/// A provider that offers a single way to sign in has exactly one slot; one that offers a
/// choice has one per mode. Splitting the hosts per slot is the point: DDownload's API key
/// belongs to `api-v2.ddownload.com` and its password to `ddownload.com`, and neither may be
/// sent to the other's host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretSlot {
    /// The `{{secret:<reference>}}` marker a resolver may expand for this slot.
    pub reference: String,
    /// Hosts this slot's secret (and the account username alongside it) may be sent to: an
    /// exact host, or `*.suffix` for the sub-domains of one (RD-120-30; MEGA's storage nodes
    /// are named per call). The username follows the exact entries only.
    pub domains: Vec<String>,
    /// The mode this slot belongs to, or `None` when the provider has only one way to sign in
    /// and the slot is therefore always available.
    pub mode: Option<CredentialMode>,
    /// Who fills this slot (RD-106-03). [`SecretFilledBy::Person`] for every slot written
    /// before the distinction existed, and for all but one of them since.
    pub filled_by: SecretFilledBy,
}

/// Who puts a value into a credential slot (RD-106-03).
///
/// The distinction exists because an OAuth provider where the person registers their own
/// application has to hold two credentials at once: the client secret they typed, and the
/// access token the sign-in obtained. One stored value cannot be both, and the sign-in must
/// not overwrite what the person typed -- doing so would destroy the very thing the next
/// renewal needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretFilledBy {
    /// The person types it into the accounts form.
    #[default]
    Person,
    /// A sign-in flow obtains it and the host keeps it beside the flow.
    Flow,
}

impl SecretSlot {
    /// Whether this slot is the active one for an account in `mode`.
    #[must_use]
    pub fn allows_mode(&self, mode: Option<CredentialMode>) -> bool {
        match self.mode {
            None => true,
            Some(own) => mode == Some(own),
        }
    }

    /// Whether a sign-in flow fills this slot rather than the person.
    #[must_use]
    pub fn is_filled_by_flow(&self) -> bool {
        self.filled_by == SecretFilledBy::Flow
    }
}

/// Where a provider row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSource {
    /// Compiled into this binary.
    Builtin,
    /// Contributed by an installed plugin manifest.
    Plugin,
}

/// One row of the provider table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSpec {
    /// Stable identifier stored as `accounts.provider` (lowercase).
    pub slug: String,
    /// Name shown in the UI when no plugin translation is loaded.
    pub display_name: String,
    pub kind: ProviderKind,
    pub credentials: CredentialKind,
    /// Whether an account for this provider must carry a non-empty username.
    pub username_required: bool,
    /// Whether the account's credential rides on the transfer itself, and in which shape.
    pub transfer_auth: TransferAuth,
    /// The credential slots this provider owns, in declaration order. Empty for a provider
    /// that stores no secret at all. The first entry is the provider's default, which is what
    /// an account written before credential modes existed falls back to.
    pub secrets: Vec<SecretSlot>,
    /// Resolver HTTP allowlist: exact hosts, or `"*.suffix"` for a host and its subdomains.
    pub request_domains: Vec<String>,
    /// Base URL whose domain (and subdomains) receive the account's cookies.
    pub cookie_scope: Option<String>,
    /// Hosts a plain download URL is mapped to this provider from (`Hoster` kind only).
    pub match_hosts: Vec<String>,
    /// `(alias_host, canonical_host)` pairs that also resolve to this provider.
    pub host_aliases: Vec<(String, String)>,
    pub source: ProviderSource,
    /// The plugin that contributed this row, and the version of it that did.
    ///
    /// Two versions of one plugin can sit installed side by side and the highest wins, which is
    /// well defined but was nowhere to be seen: a provider dropdown said "DDownload" whether the
    /// row came from 0.10.0 or 0.10.1. Carrying it here means the answer travels with the row
    /// rather than having to be looked up against a second list.
    pub plugin_id: Option<String>,
    pub plugin_version: Option<String>,
}

impl ProviderSpec {
    /// The slot named `reference`, if this provider owns it.
    #[must_use]
    pub fn secret_slot(&self, reference: &str) -> Option<&SecretSlot> {
        self.secrets.iter().find(|slot| slot.reference == reference)
    }

    /// The slot an account in `mode` uses, i.e. the first one that mode admits.
    #[must_use]
    pub fn active_secret_slot(&self, mode: Option<CredentialMode>) -> Option<&SecretSlot> {
        self.secrets.iter().find(|slot| slot.allows_mode(mode))
    }

    /// The provider's primary secret reference, for the callers that predate credential modes
    /// and only ever deal with single-slot providers.
    #[must_use]
    pub fn secret_reference(&self) -> Option<&str> {
        self.secrets.first().map(|slot| slot.reference.as_str())
    }

    /// The slot a sign-in flow fills, when this provider has one (RD-106-03).
    ///
    /// Its presence is what tells the host that this provider's access token has a place of
    /// its own and must not be written over the credential the person registered. With more
    /// than one flow slot (RD-150-09) the first declared is the access token's.
    #[must_use]
    pub fn flow_secret_slot(&self) -> Option<&SecretSlot> {
        self.secrets.iter().find(|slot| slot.is_filled_by_flow())
    }

    /// The flow-filled slot named `reference`, when it is a *part* a sign-in keeps beside its
    /// token rather than the token itself (RD-150-09).
    ///
    /// Every flow slot after the first is one: Real-Debrid's personal client id and client
    /// secret, which each renewal needs beside the refresh material. They are written by
    /// `store-flow-secret` and never by `store-oauth-token`.
    #[must_use]
    pub fn flow_part_slot(&self, reference: &str) -> Option<&SecretSlot> {
        let token = self.flow_secret_slot()?;
        self.secret_slot(reference)
            .filter(|slot| slot.is_filled_by_flow() && slot.reference != token.reference)
    }

    /// The slot the person fills, i.e. the account's own credential.
    ///
    /// The same thing [`Self::secret_reference`] names for every provider that has only one
    /// slot; separate because an OAuth provider that registers its own application has two,
    /// and only one of them is what the accounts form asks for.
    #[must_use]
    pub fn person_secret_slot(&self) -> Option<&SecretSlot> {
        self.secrets.iter().find(|slot| !slot.is_filled_by_flow())
    }

    /// The modes this provider offers, in declaration order. Empty when it offers no choice.
    #[must_use]
    pub fn credential_modes(&self) -> Vec<CredentialMode> {
        let mut modes = Vec::new();
        for mode in self.secrets.iter().filter_map(|slot| slot.mode) {
            if !modes.contains(&mode) {
                modes.push(mode);
            }
        }
        modes
    }

    /// The mode an account falls back to when it stores none: the first one declared. Accounts
    /// created before this provider grew a second mode keep working that way.
    #[must_use]
    pub fn default_credential_mode(&self) -> Option<CredentialMode> {
        self.secrets.iter().find_map(|slot| slot.mode)
    }

    /// Resolves an account's stored mode against this provider, filling in the default.
    #[must_use]
    pub fn effective_credential_mode(
        &self,
        stored: Option<CredentialMode>,
    ) -> Option<CredentialMode> {
        stored.or_else(|| self.default_credential_mode())
    }
}

/// Why a dynamic provider row was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// Another plugin already registered this slug.
    SlugTaken { slug: String, owner: String },
    /// The row claims a `{{secret:…}}` reference another provider owns. Accepting it would
    /// widen the set of hosts that provider's credential may be sent to.
    SecretReferenceTaken { reference: String, owner: String },
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SlugTaken { slug, owner } => write!(
                formatter,
                "provider slug `{slug}` is already registered by plugin {owner}"
            ),
            Self::SecretReferenceTaken { reference, owner } => write!(
                formatter,
                "secret reference `{reference}` is already owned by provider `{owner}`"
            ),
        }
    }
}

impl std::error::Error for RegisterError {}

/// A dynamic row together with the plugin id that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicProvider {
    /// Owning plugin id, so a newer version of the same plugin can replace its own row.
    pub plugin_id: String,
    pub spec: ProviderSpec,
}

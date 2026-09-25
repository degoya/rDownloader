//! Central, data-driven table of hoster/multihoster providers.
//!
//! The table has two layers. The **built-in** rows describe the resolvers compiled into
//! this binary. The **dynamic** rows are derived at startup from the `[provider]` section
//! of every installed plugin manifest, so a third-party plugin contributes a fully
//! functional provider — account creation, secret gating, HTTP allowlist, link intake —
//! without a core release. Built-ins always win a slug conflict.
//!
//! This crate sits below `rd-db`, `rd-plugin-host`, `rd-api` and `rd-core` in the dependency
//! graph. It depends on `url`, plus `serde` and `utoipa` for [`CredentialMode`] alone — that
//! enum is stored in `accounts.credential_mode` and travels over REST, and duplicating it one
//! layer up would mean two places to keep in step about which credential a password reaches.

use std::sync::{PoisonError, RwLock};

use url::Url;

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
/// Only meaningful for [`CredentialKind::LoginOrApiKey`]; every other kind has exactly one way
/// to hold an account and stores `None`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// The single secret slot holds an account password, and the resolver signs in with it.
    Login,
    /// The single secret slot holds a ready-made API key.
    ApiKey,
}

impl CredentialMode {
    /// The wire spelling, as stored in `accounts.credential_mode` and sent over REST.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::ApiKey => "api_key",
        }
    }

    /// Parses the wire spelling; unknown values are rejected rather than silently defaulted,
    /// so a typo in a restored backup cannot quietly change which host a password reaches.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "login" => Some(Self::Login),
            "api_key" => Some(Self::ApiKey),
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
    /// its own and must not be written over the credential the person registered.
    #[must_use]
    pub fn flow_secret_slot(&self) -> Option<&SecretSlot> {
        self.secrets.iter().find(|slot| slot.is_filled_by_flow())
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

static DYNAMIC: RwLock<Vec<DynamicProvider>> = RwLock::new(Vec::new());

/// Hosts whose link fragment is key material rather than an anchor (RD-110-38).
///
/// A second, deliberately separate table: this is not a provider row. A plugin that declares
/// it may be of any world -- MEGA's is a `stream-transform`, which contributes no provider at
/// all -- and what the intake needs to ask is about an address, not about an account.
static SECRET_FRAGMENT_HOSTS: RwLock<Vec<String>> = RwLock::new(Vec::new());

fn secret_fragment_write() -> std::sync::RwLockWriteGuard<'static, Vec<String>> {
    SECRET_FRAGMENT_HOSTS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Replaces the whole set, as done once at startup from every installed manifest.
///
/// Entries are lowercased here so a manifest written with capitals still matches; the
/// pattern language is the one `request_domains` already uses, an exact host or `*.suffix`.
pub fn replace_secret_fragment_hosts(hosts: Vec<String>) {
    let mut normalized: Vec<String> = hosts
        .into_iter()
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect();
    normalized.sort_unstable();
    normalized.dedup();
    *secret_fragment_write() = normalized;
}

/// Whether the fragment of `url` is a secret its provider declared, rather than an anchor.
///
/// `false` for every address in the world until a plugin says otherwise, which is what keeps
/// [`rd_core::split_candidate_url`] behaving exactly as it did for everything else.
#[must_use]
pub fn fragment_is_secret(url: &Url) -> bool {
    let Some(raw_host) = url.host_str() else {
        return false;
    };
    let host = raw_host.strip_prefix("www.").unwrap_or(raw_host);
    let host = host.to_ascii_lowercase();
    SECRET_FRAGMENT_HOSTS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|pattern| domain_matches(pattern, &host))
}

/// The dynamic table, poisoning and all.
///
/// The rows are a plain `Vec` that every writer replaces or filters wholesale, so a panic
/// under the lock cannot leave them half-built and there is nothing to protect by refusing
/// access. Treating poisoning as "skip the update" silently did the opposite of what this
/// table is for: `refresh_providers` would keep the rows of a plugin that had just been
/// removed, which is the bug the refresh exists to prevent.
fn dynamic_write() -> std::sync::RwLockWriteGuard<'static, Vec<DynamicProvider>> {
    DYNAMIC.write().unwrap_or_else(PoisonError::into_inner)
}

fn dynamic_snapshot() -> Vec<DynamicProvider> {
    DYNAMIC
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Replaces the whole dynamic layer, as done once at startup from installed manifests.
///
/// `rows` is expected newest-version-first per plugin: the first row of a plugin id wins and
/// its older versions are ignored silently. Rows that collide with a built-in slug, or with
/// another plugin's row, are dropped and returned so the caller can log them.
pub fn replace_dynamic(rows: Vec<DynamicProvider>) -> Vec<RegisterError> {
    let mut accepted: Vec<DynamicProvider> = Vec::with_capacity(rows.len());
    let mut rejected = Vec::new();
    for row in rows {
        if accepted
            .iter()
            .any(|existing| existing.plugin_id == row.plugin_id)
        {
            // An older installed version of a plugin already represented here.
            continue;
        }
        match check_row(&accepted, &row) {
            Ok(()) => accepted.push(row),
            Err(error) => rejected.push(error),
        }
    }
    *dynamic_write() = accepted;
    rejected
}

/// Adds or replaces one dynamic row, as done right after a plugin is installed.
///
/// A plugin may replace its own row (a version upgrade) but never another plugin's.
pub fn try_register_dynamic(row: DynamicProvider) -> Result<(), RegisterError> {
    let mut guard = dynamic_write();
    let others: Vec<DynamicProvider> = guard
        .iter()
        .filter(|existing| existing.plugin_id != row.plugin_id)
        .cloned()
        .collect();
    check_row(&others, &row)?;
    guard.retain(|existing| existing.plugin_id != row.plugin_id);
    guard.push(row);
    Ok(())
}

/// Drops every dynamic row owned by `plugin_id`.
pub fn unregister_dynamic(plugin_id: &str) {
    dynamic_write().retain(|existing| existing.plugin_id != plugin_id);
}

/// Rejects a dynamic row that would take another plugin's slug, or claim a secret reference
/// that already belongs to someone else.
///
/// These two rules used to sit behind a third: a plugin could never claim a slug the binary
/// had compiled in. That table is gone (RD-101-13), and with it the guarantee that a
/// well-known hoster's row was beyond a plugin's reach. What carries that weight now is
/// signature trust — a package only loads at all if a trusted key signed it — together with
/// `SecretReferenceTaken` below, which is the rule that actually mattered: it is what stops a
/// second plugin from claiming an existing credential and widening the hosts it may be sent
/// to. Slug ownership is first come, first served, in the load order `list_installed` fixes.
fn check_row(existing: &[DynamicProvider], row: &DynamicProvider) -> Result<(), RegisterError> {
    if let Some(owner) = existing
        .iter()
        .find(|other| other.spec.slug.eq_ignore_ascii_case(&row.spec.slug))
    {
        return Err(RegisterError::SlugTaken {
            slug: row.spec.slug.clone(),
            owner: owner.plugin_id.clone(),
        });
    }
    for slot in &row.spec.secrets {
        let owner = existing
            .iter()
            .map(|other| &other.spec)
            .find(|spec| spec.secret_slot(&slot.reference).is_some());
        if let Some(owner) = owner {
            return Err(RegisterError::SecretReferenceTaken {
                reference: slot.reference.clone(),
                owner: owner.slug.clone(),
            });
        }
    }
    Ok(())
}

/// All known providers, every one of them contributed by an installed plugin manifest.
///
/// Empty until the plugins are loaded, which is the point: a provider exists exactly when
/// something can resolve its links. Before RD-101-13 eleven rows were compiled into the
/// binary, so the accounts list offered hosters on an installation that had no plugins at
/// all — and an account could be created for a provider whose resolver was not there.
#[must_use]
pub fn all() -> Vec<ProviderSpec> {
    dynamic_snapshot().into_iter().map(|row| row.spec).collect()
}

/// Looks up a provider by its slug (trimmed, case-insensitive).
#[must_use]
pub fn by_slug(slug: &str) -> Option<ProviderSpec> {
    let slug = slug.trim();
    all()
        .into_iter()
        .find(|spec| spec.slug.eq_ignore_ascii_case(slug))
}

/// Maps a plain download URL to the `Hoster`-kind provider that serves its domain, stripping
/// a leading `www.` and consulting each provider's `host_aliases`. Multihosters never claim a
/// URL this way: they resolve links for other providers' domains, not their own.
#[must_use]
pub fn provider_for_url(url: &Url) -> Option<ProviderSpec> {
    let raw_host = url.host_str()?;
    let host = raw_host.strip_prefix("www.").unwrap_or(raw_host);
    all().into_iter().find(|spec| {
        spec.kind == ProviderKind::Hoster
            && (spec.match_hosts.iter().any(|value| value == host)
                || spec.host_aliases.iter().any(|(alias, _)| alias == host))
    })
}

/// Whether `reference` is a secret marker `slug`'s resolver is allowed to expand.
///
/// Ownership only: a provider with two modes owns both of its references regardless of which
/// one the account currently uses. Narrowing that to the account's active mode is the caller's
/// job, because only the caller knows the account (see `rd_plugin_host`'s `named_secret`).
#[must_use]
pub fn secret_reference_allowed(slug: &str, reference: &str) -> bool {
    by_slug(slug).is_some_and(|spec| spec.secret_slot(reference).is_some())
}

/// Whether the secret/credential identified by `reference` may be sent to `url`'s host.
#[must_use]
pub fn secret_domain_allowed(reference: &str, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    all().iter().any(|spec| {
        spec.secret_slot(reference).is_some_and(|slot| {
            slot.domains
                .iter()
                .any(|domain| slot_domain_matches(domain, host))
        })
    })
}

/// Whether everything the sandbox pattern `reach` lets a plugin talk to is somewhere the
/// secret behind `reference` may itself be sent (RD-120-20, ADR 0020 point 7).
///
/// The question the key-derivation grant asks, and it is a question about *patterns*, not
/// hosts: a plugin that reaches `*.example.test` can talk to every sub-domain there is, so it
/// passes only where the slot names that wildcard (or a wider one) too. An exact slot entry
/// never covers a wildcard reach -- until RD-120-30 the check tested the wildcard's bare
/// suffix instead, which answered for one host the plugin could reach and none of the others.
#[must_use]
pub fn secret_reach_allowed(reference: &str, reach: &str) -> bool {
    all().iter().any(|spec| {
        spec.secret_slot(reference).is_some_and(|slot| {
            slot.domains
                .iter()
                .any(|domain| pattern_covers(domain, reach))
        })
    })
}

/// Whether a slot's host entry admits `host`: exactly, or as a sub-domain of a `*.suffix`
/// entry -- never the bare suffix, which is the reading the sandbox applies to `net_http`.
fn slot_domain_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host.ends_with(&format!(".{suffix}")),
        None => host == pattern,
    }
}

/// Whether the slot entry `slot` covers every host the reach pattern `reach` covers.
fn pattern_covers(slot: &str, reach: &str) -> bool {
    match (slot.strip_prefix("*."), reach.strip_prefix("*.")) {
        (_, None) => reach != "*" && slot_domain_matches(slot, reach),
        (Some(slot), Some(reach)) => reach == slot || reach.ends_with(&format!(".{slot}")),
        (None, Some(_)) => false,
    }
}

/// Whether `host` is allowed as a resolver HTTP target, unioned over every provider's
/// `request_domains`. Supports exact hosts and `"*.suffix"` subdomain patterns.
#[must_use]
pub fn request_domain_allowed(host: &str) -> bool {
    all()
        .iter()
        .flat_map(|spec| spec.request_domains.iter())
        .any(|pattern| domain_matches(pattern, host))
}

fn domain_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix('*') {
        Some(suffix) => host.ends_with(suffix),
        None => host == pattern,
    }
}

/// The base URL whose domain (and subdomains) should receive `slug`'s account cookies.
#[must_use]
pub fn cookie_scope(slug: &str) -> Option<Url> {
    by_slug(slug)?
        .cookie_scope
        .and_then(|base| Url::parse(&base).ok())
}

/// All `(alias_host, canonical_host)` pairs across every provider, for rewriting short-link
/// and mirror hosts to their canonical resolver domain at link intake.
#[must_use]
pub fn host_aliases() -> Vec<(String, String)> {
    all()
        .into_iter()
        .flat_map(|spec| spec.host_aliases.into_iter())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        value.parse().expect("url")
    }

    /// Serialises the tests that mutate the process-wide dynamic layer.
    static DYNAMIC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn dynamic_row(plugin_id: &str, slug: &str) -> DynamicProvider {
        DynamicProvider {
            plugin_id: plugin_id.to_owned(),
            spec: ProviderSpec {
                slug: slug.to_owned(),
                display_name: "Fixture".to_owned(),
                kind: ProviderKind::Hoster,
                credentials: CredentialKind::ApiKey,
                username_required: false,
                transfer_auth: TransferAuth::None,
                secrets: vec![SecretSlot {
                    reference: format!("{slug}_api_key"),
                    domains: vec!["api.fixture.test".to_owned()],
                    mode: None,
                    filled_by: SecretFilledBy::Person,
                }],
                request_domains: vec!["fixture.test".to_owned(), "*.fixture.test".to_owned()],
                cookie_scope: Some("https://fixture.test/".to_owned()),
                match_hosts: vec!["fixture.test".to_owned()],
                host_aliases: vec![("fixture.example".to_owned(), "fixture.test".to_owned())],
                source: ProviderSource::Plugin,
                plugin_id: Some(plugin_id.to_owned()),
                plugin_version: Some("1.0.0".to_owned()),
            },
        }
    }

    /// A multihoster fixture: it accepts any host and therefore claims none.
    fn multihoster_row(plugin_id: &str, slug: &str) -> DynamicProvider {
        let mut row = dynamic_row(plugin_id, slug);
        row.spec.kind = ProviderKind::Multihoster;
        row.spec.match_hosts = Vec::new();
        row
    }

    // -- lookup basics --------------------------------------------------
    //
    // These used to be asserted against the compiled-in hoster rows. With the table gone
    // (RD-101-13) they are stated against fixtures, which is what they were always about:
    // how the lookup treats a slug and a host, not which hosters happen to ship.

    #[test]
    fn by_slug_is_trimmed_and_case_insensitive() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
        assert_eq!(by_slug(" Fixture ").expect("spec").slug, "fixture");
        assert_eq!(by_slug("FIXTURE").expect("spec").slug, "fixture");
        assert!(by_slug("nonexistent").is_none());
        replace_dynamic(Vec::new());
    }

    #[test]
    fn provider_for_url_strips_www_and_resolves_aliases() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
        for address in [
            "https://fixture.test/abc",
            "https://www.fixture.test/abc",
            "https://fixture.example/abc",
        ] {
            assert_eq!(
                provider_for_url(&url(address)).expect("spec").slug,
                "fixture",
                "{address}"
            );
        }
        assert!(provider_for_url(&url("https://example.com/file")).is_none());
        replace_dynamic(Vec::new());
    }

    #[test]
    fn multihosters_never_claim_a_url() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(vec![multihoster_row("plugin-m", "fixture")]);
        assert!(provider_for_url(&url("https://fixture.test/")).is_none());
        for spec in all() {
            if spec.kind == ProviderKind::Multihoster {
                assert!(spec.match_hosts.is_empty(), "{} has match_hosts", spec.slug);
            }
        }
        replace_dynamic(Vec::new());
    }

    /// Nothing is a provider until a plugin says so.
    ///
    /// The eleven compiled-in rows are gone; an installation without plugins therefore offers
    /// no providers at all, instead of offering hosters it cannot resolve.
    #[test]
    fn without_plugins_there_are_no_providers() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(Vec::new());
        assert!(all().is_empty());
        assert!(by_slug("ddownload").is_none());
        assert!(!request_domain_allowed("ddownload.com"));
    }

    #[test]
    fn every_row_comes_from_a_plugin() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
        assert!(
            all()
                .iter()
                .all(|spec| spec.source == ProviderSource::Plugin)
        );
        replace_dynamic(Vec::new());
    }

    // -- dynamic layer --------------------------------------------------

    #[test]
    fn plugin_rows_extend_every_gate() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        assert!(by_slug("fixture").is_none());
        assert!(!request_domain_allowed("fixture.test"));

        let rejected = replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
        assert!(rejected.is_empty());

        let spec = by_slug("fixture").expect("dynamic provider is visible");
        assert_eq!(spec.source, ProviderSource::Plugin);
        assert!(request_domain_allowed("fixture.test"));
        assert!(request_domain_allowed("cdn.fixture.test"));
        assert!(secret_reference_allowed("fixture", "fixture_api_key"));
        assert!(secret_domain_allowed(
            "fixture_api_key",
            &url("https://api.fixture.test/account")
        ));
        assert_eq!(
            provider_for_url(&url("https://fixture.test/file"))
                .expect("spec")
                .slug,
            "fixture"
        );
        assert_eq!(
            cookie_scope("fixture").expect("scope").as_str(),
            "https://fixture.test/"
        );
        assert!(
            host_aliases()
                .iter()
                .any(|(alias, _)| alias == "fixture.example")
        );

        replace_dynamic(Vec::new());
        assert!(by_slug("fixture").is_none());
        assert!(!request_domain_allowed("fixture.test"));
    }

    #[test]
    fn a_slug_belongs_to_the_plugin_that_claimed_it_first() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(Vec::new());
        try_register_dynamic(dynamic_row("plugin-a", "fixture")).expect("first claim wins");

        let error = try_register_dynamic(dynamic_row("plugin-b", "fixture"))
            .expect_err("a second plugin cannot take the slug");
        assert!(matches!(error, RegisterError::SlugTaken { .. }));

        // The same plugin replacing its own row is an upgrade, not a conflict.
        let mut upgraded = dynamic_row("plugin-a", "fixture");
        upgraded.spec.display_name = "Fixture 2".to_owned();
        try_register_dynamic(upgraded).expect("self-replacement is allowed");
        assert_eq!(by_slug("fixture").expect("spec").display_name, "Fixture 2");

        unregister_dynamic("plugin-a");
        assert!(by_slug("fixture").is_none());
        replace_dynamic(Vec::new());
    }

    /// The rule that carries the weight now that no slug is reserved.
    ///
    /// A plugin cannot claim a credential another provider already owns. That is what stops it
    /// from widening the set of hosts an existing secret may be sent to — the concrete danger
    /// the built-in reservation used to cover, and the only half of it that was load-bearing.
    /// The rest is signature trust: an untrusted package does not load at all.
    #[test]
    fn a_plugin_cannot_claim_another_providers_secret_reference() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);

        let mut thief = dynamic_row("plugin-evil", "lookalike");
        // The owner's credential would otherwise become sendable to this plugin's hosts.
        thief.spec.secrets[0].reference = "fixture_api_key".to_owned();
        thief.spec.secrets[0].domains = vec!["evil.test".to_owned()];
        let error = try_register_dynamic(thief).expect_err("the credential is taken");
        assert!(matches!(error, RegisterError::SecretReferenceTaken { .. }));

        // The owner keeps its own hosts, and the thief's never became valid for the slot.
        assert!(secret_domain_allowed(
            "fixture_api_key",
            &url("https://api.fixture.test/account")
        ));
        assert!(!secret_domain_allowed(
            "fixture_api_key",
            &url("https://evil.test/account")
        ));
        replace_dynamic(Vec::new());
    }

    #[test]
    fn older_versions_of_the_same_plugin_do_not_conflict_with_themselves() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        let mut newest = dynamic_row("plugin-a", "fixture");
        newest.spec.display_name = "Fixture 2".to_owned();
        let older = dynamic_row("plugin-a", "fixture");

        let rejected = replace_dynamic(vec![newest, older]);
        assert!(
            rejected.is_empty(),
            "a plugin's own older version is not a conflict"
        );
        assert_eq!(by_slug("fixture").expect("spec").display_name, "Fixture 2");
        replace_dynamic(Vec::new());
    }

    // -- gate behaviour --------------------------------------------------
    //
    // What each gate does with a pattern, stated against fixtures. The parity assertions for
    // the actual hosters moved to `rd-plugin-host/tests/bundled_providers.rs` when the built-in
    // table went (RD-101-13): this crate no longer knows a single hoster, and the facts it used
    // to assert are now the manifests' to keep.

    #[test]
    fn request_domain_allowed_matches_exact_and_wildcard_suffixes() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
        assert!(request_domain_allowed("fixture.test"));
        assert!(request_domain_allowed("cdn123.fixture.test"));
        // A suffix match must not be a substring match.
        assert!(!request_domain_allowed("notfixture.test"));
        assert!(!request_domain_allowed("evil.com"));
        replace_dynamic(Vec::new());
    }

    #[test]
    fn host_aliases_unions_every_provider() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        let mut second = dynamic_row("plugin-b", "other");
        second.spec.host_aliases = vec![("other.example".to_owned(), "other.test".to_owned())];
        second.spec.secrets[0].reference = "other_api_key".to_owned();
        replace_dynamic(vec![dynamic_row("plugin-a", "fixture"), second]);

        let aliases = host_aliases();
        assert!(
            aliases
                .iter()
                .any(|(alias, canonical)| alias == "fixture.example" && canonical == "fixture.test")
        );
        assert!(
            aliases
                .iter()
                .any(|(alias, canonical)| alias == "other.example" && canonical == "other.test")
        );
        replace_dynamic(Vec::new());
    }

    /// A host is declared, and everything else on earth is not (RD-110-38).
    #[test]
    fn only_a_declared_host_has_a_secret_fragment() {
        let _guard = DYNAMIC_LOCK.lock().expect("lock");
        replace_secret_fragment_hosts(vec!["MEGA.nz".to_owned(), "*.share.test".to_owned()]);

        let declared =
            |address: &str| fragment_is_secret(&address.parse::<Url>().expect("an address"));
        // Case and a leading `www.` are read the way every other host lookup here reads them.
        assert!(declared("https://mega.nz/file/abc#key"));
        assert!(declared("https://www.mega.nz/file/abc#key"));
        // A fragment is not required to answer: the question is about the host.
        assert!(declared("https://mega.nz/file/abc"));
        // `*.suffix` covers sub-domains only, never the bare suffix -- the same reading the
        // sandbox allowlist uses.
        assert!(declared("https://one.share.test/x#k"));
        assert!(!declared("https://share.test/x#k"));
        // And a suffix match is not a substring match.
        assert!(!declared("https://notmega.nz/file/abc#key"));
        assert!(!declared("https://example.com/a#b"));
        assert!(!declared("magnet:?xt=urn:btih:0123456789abcdef"));

        // Removing the plugin removes the declaration with it.
        replace_secret_fragment_hosts(Vec::new());
        assert!(!declared("https://mega.nz/file/abc#key"));
    }

    /// The reach half of ADR 0020 point 7, as patterns (RD-120-30). A wildcard reach is
    /// covered only by a wildcard at least as wide; an exact entry covers exactly its host.
    #[test]
    fn a_wildcard_reach_needs_a_wildcard_slot_and_never_its_bare_suffix() {
        assert!(pattern_covers("api.example.test", "api.example.test"));
        assert!(pattern_covers(
            "*.nodes.example.test",
            "*.nodes.example.test"
        ));
        assert!(pattern_covers("*.example.test", "*.nodes.example.test"));
        assert!(pattern_covers("*.example.test", "a.example.test"));
        // The hole the old apex test had: `*.nodes.example.test` reaches every node, and an
        // entry for the bare `nodes.example.test` answers for none of them.
        assert!(!pattern_covers(
            "nodes.example.test",
            "*.nodes.example.test"
        ));
        assert!(!pattern_covers("*.nodes.example.test", "*.example.test"));
        assert!(!pattern_covers("*.example.test", "example.test"));
        assert!(!pattern_covers("*.example.test", "*"));
        // Sending follows the same reading: a sub-domain, never the bare suffix.
        assert!(slot_domain_matches(
            "*.nodes.example.test",
            "n1.nodes.example.test"
        ));
        assert!(!slot_domain_matches(
            "*.nodes.example.test",
            "nodes.example.test"
        ));
        assert!(!slot_domain_matches(
            "*.nodes.example.test",
            "evilnodes.example.test"
        ));
    }
}

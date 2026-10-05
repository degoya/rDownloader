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

mod host_pattern;
mod spec;

use std::sync::{PoisonError, RwLock};

use url::Url;

pub use host_pattern::{WildcardApex, host_key, host_pattern_matches};
pub use spec::{
    CredentialKind, CredentialMode, DynamicProvider, ProviderKind, ProviderSource, ProviderSpec,
    RegisterError, SecretFilledBy, SecretSlot, TransferAuth,
};

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
    let host = host_key(raw_host);
    SECRET_FRAGMENT_HOSTS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|pattern| slot_domain_matches(pattern, &host))
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
    let host = host_key(url.host_str()?);
    let host = host.as_str();
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
///
/// Only tests ask it this way (RD-191-06, PLUG-15); the service decides through the account.
#[cfg(any(test, feature = "test-support"))]
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
    host_pattern_matches(pattern, host, WildcardApex::Excluded)
}

/// Whether the slot entry `slot` covers every host the reach pattern `reach` covers.
fn pattern_covers(slot: &str, reach: &str) -> bool {
    match (slot.strip_prefix("*."), reach.strip_prefix("*.")) {
        (_, None) => reach != "*" && slot_domain_matches(slot, reach),
        // `*.x` covers `*.x` and `*.a.x`: the reach's suffix is the slot's or below it.
        (Some(_), Some(reach)) => host_pattern_matches(slot, reach, WildcardApex::Included),
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
        .any(|pattern| host_pattern_matches(pattern, host, WildcardApex::Excluded))
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
mod tests;

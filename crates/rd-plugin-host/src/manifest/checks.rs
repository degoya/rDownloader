//! The per-section checks `validate_manifest` runs: the metadata, the provider and its credential
//! slots, a notifier's settings, slugs, bounded text and domain patterns.
//!
//! Split out of `manifest.rs` (PLUG-21).

use anyhow::{Context, Result, bail};

use super::{
    CredentialKindManifest, CredentialModeManifest, MAX_AUTHOR_CHARS, MAX_DESCRIPTION_CHARS,
    MAX_LICENSE_CHARS, MAX_SLUG_CHARS, MAX_URL_CHARS, MIN_SLUG_CHARS, PluginMetadata,
    ProviderKindManifest, ProviderManifest, SecretFilledByManifest, SecretSlotManifest,
    SettingManifest, TransferAuthManifest,
};

pub(super) fn validate_metadata(metadata: &PluginMetadata) -> Result<()> {
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

pub(super) fn validate_provider(provider: &ProviderManifest, domains: &[String]) -> Result<()> {
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
        if slots
            .iter()
            .any(|slot| slot.mode == Some(CredentialModeManifest::OAuth))
        {
            bail!(
                "provider.credentials = \"login_or_api_key\" offers the modes login and api_key only"
            );
        }
    } else if provider.credentials == CredentialKindManifest::OAuthOrApiKey {
        validate_oauth_or_api_key(&slots)?;
    } else if slots.iter().any(|slot| slot.mode.is_some()) {
        bail!(
            "provider.secrets may only declare a mode when credentials = \"login_or_api_key\" or \"oauth_or_api_key\""
        );
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
    if flow_slots > 0 && provider.credentials != CredentialKindManifest::OAuthOrApiKey {
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

/// The slots of a provider that signs in with a code or takes an API key (RD-150-09).
///
/// Each mode has to be complete on its own, and neither may borrow from the other: the API key
/// is one slot the person fills, and signing in fills everything of its mode -- the token
/// first, then any named part the renewal needs beside it. A typed value in the sign-in mode
/// would be a credential the form never asks for, and a flow slot in the key mode a sign-in
/// that mode never runs.
fn validate_oauth_or_api_key(slots: &[SecretSlotManifest]) -> Result<()> {
    if slots.iter().any(|slot| slot.mode.is_none()) {
        bail!(
            "provider.credentials = \"oauth_or_api_key\" requires a mode on every provider.secrets entry"
        );
    }
    if slots
        .iter()
        .any(|slot| slot.mode == Some(CredentialModeManifest::Login))
    {
        bail!(
            "provider.credentials = \"oauth_or_api_key\" offers the modes oauth and api_key only"
        );
    }
    let api_key: Vec<_> = slots
        .iter()
        .filter(|slot| slot.mode == Some(CredentialModeManifest::ApiKey))
        .collect();
    if api_key.len() != 1 || api_key[0].filled_by != SecretFilledByManifest::Person {
        bail!(
            "provider.credentials = \"oauth_or_api_key\" needs exactly one api_key entry, filled by the person"
        );
    }
    let sign_in: Vec<_> = slots
        .iter()
        .filter(|slot| slot.mode == Some(CredentialModeManifest::OAuth))
        .collect();
    if sign_in.is_empty()
        || sign_in
            .iter()
            .any(|slot| slot.filled_by != SecretFilledByManifest::Flow)
    {
        bail!(
            "provider.credentials = \"oauth_or_api_key\" needs oauth entries, each with filled_by = \"flow\""
        );
    }
    Ok(())
}

/// Most settings one destination declares, and most values one setting offers.
const MAX_SETTINGS: usize = 16;
const MAX_SETTING_CHOICES: usize = 32;

/// `[[extension.settings]]`: names that can be a key and a code, values that can be offered.
pub(super) fn validate_settings(settings: &[SettingManifest]) -> Result<()> {
    if settings.len() > MAX_SETTINGS {
        bail!("extension.settings declares more than {MAX_SETTINGS} settings");
    }
    let mut names = Vec::new();
    for setting in settings {
        let name = setting.name.as_str();
        if name.is_empty()
            || name.len() > MAX_SLUG_CHARS
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            bail!("extension.settings name `{name}` must be 1-{MAX_SLUG_CHARS} of a-z, 0-9 and _");
        }
        if names.contains(&name) {
            bail!("extension.settings declares `{name}` twice");
        }
        names.push(name);
        if setting.choices.is_empty() || setting.choices.len() > MAX_SETTING_CHOICES {
            bail!("extension.settings `{name}` needs 1-{MAX_SETTING_CHOICES} choices");
        }
        for (index, choice) in setting.choices.iter().enumerate() {
            bounded_text("extension.settings choice", choice, MAX_SLUG_CHARS)?;
            if choice.trim() != choice || setting.choices[..index].contains(choice) {
                bail!("extension.settings `{name}` offers `{choice}` badly or twice");
            }
        }
        if let Some(default) = &setting.default
            && !setting.choices.contains(default)
        {
            bail!("extension.settings `{name}` defaults to `{default}`, which it does not offer");
        }
    }
    Ok(())
}

pub(super) fn validate_slug(slug: &str) -> Result<()> {
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

pub(super) fn bounded_text(field: &str, value: &str, limit: usize) -> Result<()> {
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
pub(super) fn host_covered(host: &str, domains: &[String]) -> bool {
    domains
        .iter()
        .any(|domain| rd_core::host_pattern_matches(domain, host, rd_core::WildcardApex::Excluded))
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
    // `*` only as a leading `*.` before at least two labels, as a site rule's `match.hosts`
    // reads it (`rd-siterules`, `text::is_host_pattern`; RA-HOST-06): `*foo.com` and
    // `cdn.*.com` matched nothing at request time, and `*.com` covered a whole top-level domain.
    let host = domain.strip_prefix("*.").unwrap_or(domain);
    let labels = host.split('.').collect::<Vec<_>>();
    if host.contains('*') {
        bail!("plugin domain {domain} may carry `*` only as a leading `*.`");
    }
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) {
        bail!("plugin domain {domain} needs at least two labels after any `*.`");
    }
    // A plugin's reach is the public internet, and an address names no service a plugin author
    // could stand behind (RA-HOST-01): `127.0.0.1`, `169.254.169.254` and `localhost` reached
    // this machine, and the service's API trusts a request from there. The guard at request
    // time refuses them anyway; refusing them here tells the author at packaging time.
    if matches!(
        url::Host::parse(host),
        Ok(url::Host::Ipv4(_) | url::Host::Ipv6(_))
    ) || labels
        .last()
        .is_some_and(|last| last.bytes().all(|byte| byte.is_ascii_digit()))
    {
        bail!("plugin domain {domain} is an address, not a name");
    }
    if host == "localhost" || host.ends_with(".localhost") {
        bail!("plugin domain {domain} names this machine");
    }
    Ok(())
}

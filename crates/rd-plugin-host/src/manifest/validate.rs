//! `validate_manifest` and the two whole-manifest checks it runs: which sections a plugin type
//! carries (`validate_shape`) and whether the grants agree with the provider
//! (`validate_capabilities`). The order of the checks is the order of the refusals an author
//! sees.
//!
//! Split out of `manifest.rs` (PLUG-21); the per-section checks live in `checks.rs`.

use anyhow::{Context, Result, bail};

use super::checks::{
    bounded_text, validate_domain_pattern, validate_metadata, validate_provider, validate_settings,
    validate_slug,
};
use super::{
    Capabilities, CredentialKindManifest, ExtensionManifest, MANIFEST_VERSION, MAX_FUEL,
    MAX_MEMORY_BYTES, MAX_RESPONSE_BYTES, MAX_SLUG_CHARS, MAX_TIMEOUT_MILLISECONDS,
    MAX_WAIT_BUDGET_MILLISECONDS, ManifestRejection, PluginManifest, PluginType, ProviderManifest,
    REMOTE_JOB_CONTAINERS, SUPPORTED_API_VERSIONS, SecretFilledByManifest, safe_segment,
};

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
        PluginType::Transfer => validate_transfer_shape(manifest)?,
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
        | PluginType::StreamTransform => validate_extension_shape(manifest)?,
        PluginType::Unknown(_) => unreachable!("unknown types are refused before this point"),
    }
    validate_type_bound_fields(manifest)?;
    validate_net_stream(manifest)
}

/// A transfer backend: a `[transfer]` section with its slug and schemes, no `[provider]`, and
/// the `net_stream` grant it speaks its protocol through.
fn validate_transfer_shape(manifest: &PluginManifest) -> Result<()> {
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
    Ok(())
}

/// The shape every `[extension]` type shares, then what each of them needs on its own.
fn validate_extension_shape(manifest: &PluginManifest) -> Result<()> {
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
        || (manifest.provider.is_some() && manifest.plugin_type != PluginType::StreamTransform)
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
    validate_outbound_grant(manifest)?;
    validate_extension_fields(manifest, extension)
}

/// The `[extension]` types that exist to reach a provider or a destination need a way out.
fn validate_outbound_grant(manifest: &PluginManifest) -> Result<()> {
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
    if manifest.plugin_type == PluginType::Crawler && manifest.capabilities.net_http.is_none() {
        bail!("a crawler needs at least one capabilities.net_http domain");
    }
    // Same rule, same reason, for the eleventh type (RD-107-06): a remote job that
    // cannot reach its provider cannot submit, poll, choose or delete anything. A
    // manifest asking for no way out is a mistake and not a plugin that submits to
    // nowhere.
    if manifest.plugin_type == PluginType::RemoteJob && manifest.capabilities.net_http.is_none() {
        bail!("a remote-job plugin needs at least one capabilities.net_http domain");
    }
    // And the twelfth (RD-110-33): a plugin that cannot reach its provider cannot
    // learn the address or the key schedule it exists to answer with.
    if manifest.plugin_type == PluginType::StreamTransform
        && manifest.capabilities.net_http.is_none()
    {
        bail!("a stream-transform plugin needs at least one capabilities.net_http domain");
    }
    Ok(())
}

/// The `[extension]` fields only one type may carry: settings, containers and `generic`.
fn validate_extension_fields(
    manifest: &PluginManifest,
    extension: &ExtensionManifest,
) -> Result<()> {
    // Only a target of a notification destination has settings to store; on any
    // other type the list would be a promise nothing keeps (RD-170-09).
    if !extension.settings.is_empty() && manifest.plugin_type != PluginType::Notifier {
        bail!(
            "a {} manifest must not declare extension.settings",
            manifest.plugin_type.as_str()
        );
    }
    validate_settings(&extension.settings)?;
    // Only a remote job is ever handed a container, and a format nothing offers would be
    // a promise nobody reads (RD-191-13).
    if !extension.containers.is_empty() && manifest.plugin_type != PluginType::RemoteJob {
        bail!(
            "a {} manifest must not declare extension.containers",
            manifest.plugin_type.as_str()
        );
    }
    for (index, format) in extension.containers.iter().enumerate() {
        if !REMOTE_JOB_CONTAINERS.contains(&format.as_str())
            || extension.containers[..index].contains(format)
        {
            bail!(
                "extension.containers names `{format}` badly or twice; one of {}",
                REMOTE_JOB_CONTAINERS.join(", ")
            );
        }
    }
    // Only a crawler is ever asked in an order, so on any other type the flag would
    // be a claim about behaviour that does not exist.
    if extension.generic && manifest.plugin_type != PluginType::Crawler {
        bail!(
            "a {} manifest must not declare extension.generic",
            manifest.plugin_type.as_str()
        );
    }
    Ok(())
}

/// The grants and sections only some types may carry: `key_derivation`, `oauth_flows` and
/// `[extension]`.
fn validate_type_bound_fields(manifest: &PluginManifest) -> Result<()> {
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
    Ok(())
}

/// The `net_stream` grant, whichever type asks for it: hosts and ports, no port 0.
fn validate_net_stream(manifest: &PluginManifest) -> Result<()> {
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
    // The parts a sign-in keeps beside its token (RD-150-09) belong to the sign-in plugin the
    // same way: Real-Debrid's personal client secret is expanded by `realdebrid-auth` alone, and
    // the resolver that owns the row has no business holding it. The token's slot -- the first
    // flow slot -- is still the resolver's to grant, because the resolver is what sends it.
    let token_slot = provider
        .secret_slots()
        .into_iter()
        .find(|slot| slot.filled_by == SecretFilledByManifest::Flow)
        .map(|slot| slot.reference);
    for slot in provider.secret_slots() {
        if sign_in_slot_is_a_siblings && slot.filled_by == SecretFilledByManifest::Person {
            continue;
        }
        let is_flow_part = provider.credentials == CredentialKindManifest::OAuthOrApiKey
            && slot.filled_by == SecretFilledByManifest::Flow
            && token_slot.as_ref() != Some(&slot.reference);
        if is_flow_part {
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

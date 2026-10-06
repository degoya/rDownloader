//! A plugin's own bundled `manifest.toml`, read into its resolver metadata.

use rd_core::PluginId;
use serde::Deserialize;

use super::ResolverMetadata;

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

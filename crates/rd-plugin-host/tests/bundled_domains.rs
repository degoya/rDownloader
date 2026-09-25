//! Keeps every bundled plugin's sandbox allowlist reachable through the registry gate.
//!
//! A plugin declares the hosts it may contact in `manifest.toml`'s `domains`, but every
//! resolver request is additionally checked against the union of the provider registry's
//! `request_domains` (`rd_provider_registry::request_domain_allowed`, enforced in
//! `native::expand::validate_request_domain`). A host that only one of the two lists allows
//! is dead configuration: the manifest promises a request the gate then refuses at runtime
//! with `plugin.target_not_allowed`, which is exactly how the account-less flows for
//! 1fichier's mirror domains and Keep2Share's delivery subdomains failed before this test
//! existed.

use std::path::PathBuf;

/// Bundled plugin directories, alongside `crates/`.
fn plugin_manifests() -> Vec<(String, rd_plugin_host::PluginManifest)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .canonicalize()
        .expect("plugins directory");
    let mut manifests: Vec<(String, rd_plugin_host::PluginManifest)> = std::fs::read_dir(root)
        .expect("read plugins")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.toml").is_file())
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .expect("plugin directory name")
                .to_owned();
            let text = std::fs::read_to_string(path.join("manifest.toml")).expect("manifest");
            (name, toml::from_str(&text).expect("parse manifest"))
        })
        .collect();
    manifests.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(manifests.len() >= 12, "expected the bundled plugins");
    manifests
}

/// A concrete host that `pattern` stands for, so the gate can be asked about it.
fn sample_host(pattern: &str) -> String {
    match pattern.strip_prefix("*.") {
        Some(suffix) => format!("delivery-1.{suffix}"),
        None => pattern.to_owned(),
    }
}

/// Puts every bundled plugin's provider row into the registry's dynamic layer, the way startup
/// does from the installed manifests.
///
/// Since RD-101-13 this is the only layer there is: no provider row is compiled into the
/// binary, so every bundled resolver's domains reach the gate solely through its manifest.
/// Testing without it would measure a state that never exists at runtime.
fn register_bundled_providers() {
    let rows: Vec<rd_provider_registry::DynamicProvider> = plugin_manifests()
        .into_iter()
        .filter_map(|(_, manifest)| rd_plugin_host::provider_spec_from_manifest(&manifest))
        .collect();
    let rejected = rd_provider_registry::replace_dynamic(rows);
    // Every bundled row has to be accepted now. Until RD-101-13 most of them were refused here
    // because a built-in owned the slug, and the test had to allow for that; with the built-in
    // table gone, a refusal means a bundled provider is simply missing — two plugins fighting
    // over a slug, or over a secret reference.
    assert!(
        rejected.is_empty(),
        "the registry refused a bundled provider row: {rejected:?}"
    );
}

#[test]
fn every_manifest_domain_is_reachable_through_the_registry_gate() {
    register_bundled_providers();
    for (plugin, manifest) in plugin_manifests() {
        // Only what the gate actually applies to. A resolver serves a provider, so the
        // registry's union of request domains is a second net under its manifest. An
        // extension type serves no provider — ntfy, Discord and Telegram are in no
        // provider's list and never will be — so for those the manifest is the whole answer
        // and this check would be measuring them against a rule they are not under.
        if manifest.plugin_type != rd_plugin_host::PluginType::Resolver {
            continue;
        }
        for pattern in manifest.domains() {
            // A bare `*` (a multihoster's intake wildcard) is not a request target.
            if pattern == "*" {
                continue;
            }
            let host = sample_host(pattern);
            assert!(
                rd_provider_registry::request_domain_allowed(&host),
                "{plugin}'s manifest allows {pattern}, but the registry gate refuses {host} \
                 — every resolver request to it would fail with plugin.target_not_allowed"
            );
        }
    }
}

/// The account-less flows that motivated the test, pinned by name so a later registry edit
/// cannot quietly take them away again.
#[test]
fn the_free_flows_delivery_hosts_stay_allowed() {
    register_bundled_providers();
    for host in [
        // Keep2Share serves the captcha image and the file from delivery subdomains.
        "k2s.cc",
        "fs1.k2s.cc",
        // A 1fichier link keeps the uploader's own mirror domain.
        "1fichier.com",
        "alterupload.com",
        "desfichiers.com",
        "tenvoi.com",
        // DDownload delivers through its own CDN brand.
        "eu-hydra5.zeuscdn.org",
        // Rapidgator and Nitroflare deliver from subdomains of their main domain.
        "pr1.rapidgator.net",
        "cdn.nitroflare.com",
        // MediaFire's direct link points at a numbered delivery host (measured 2026-09-21).
        "download1514.mediafire.com",
        "download2269.mediafire.com",
    ] {
        assert!(
            rd_provider_registry::request_domain_allowed(host),
            "{host} must stay allowed for the free download flows"
        );
    }
}

/// The gate must still refuse anything no provider claims.
#[test]
fn unrelated_hosts_stay_refused() {
    register_bundled_providers();
    for host in ["example.com", "evil.test", "notk2s.cc", "k2s.cc.evil.test"] {
        assert!(
            !rd_provider_registry::request_domain_allowed(host),
            "{host} must not be an allowed resolver target"
        );
    }
}

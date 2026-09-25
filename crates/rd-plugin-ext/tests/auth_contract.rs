//! The authentication contract, exercised against the three bundled sign-in plugins.
//!
//! The promise this type has to keep is unusual: its whole job is to put an address in front
//! of somebody and ask them to sign in there. A plugin that could name any address would be a
//! signed, installed phishing page — so the manifest gate on the verification URL is what most
//! of this file is about.

use rd_plugin_host::{PluginManifest, artifact::component, extension::AuthProvider};

const DEBRIDLINK: &str = include_str!("../../../plugins/debridlink-auth/manifest.toml");
const ALLDEBRID: &str = include_str!("../../../plugins/alldebrid-auth/manifest.toml");
const PREMIUMIZE: &str = include_str!("../../../plugins/premiumize-auth/manifest.toml");

fn manifest(source: &str) -> PluginManifest {
    toml::from_str(source).expect("bundled manifest")
}

#[tokio::test]
async fn each_plugin_compiles_against_the_auth_world() {
    // Which also proves the `credentials` import links: no other plugin type may name it, so
    // a build that succeeds here and fails elsewhere is the type binding doing its job.
    for (crate_name, source) in [
        ("rd-plugin-debridlink-auth", DEBRIDLINK),
        ("rd-plugin-alldebrid-auth", ALLDEBRID),
        ("rd-plugin-premiumize-auth", PREMIUMIZE),
    ] {
        let bytes = component(crate_name);
        AuthProvider::new(manifest(source), &bytes, None)
            .unwrap_or_else(|error| panic!("{crate_name} does not satisfy the world: {error}"));
    }
}

/// Registers every bundled provider row, the way startup does from the installed manifests.
///
/// Since RD-101-13 a provider exists exactly while its plugin is installed; nothing is
/// compiled in. A bare test process therefore knows no provider at all. An auth plugin only
/// *claims* a slug — the plugin that *defines* it is the matching resolver — so the whole
/// bundled set has to be registered before the claim below can be checked against it.
fn register_bundled_providers() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .canonicalize()
        .expect("plugins directory");
    let rows: Vec<_> = std::fs::read_dir(root)
        .expect("read plugins")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.toml").is_file())
        .map(|path| {
            let text = std::fs::read_to_string(path.join("manifest.toml")).expect("manifest");
            toml::from_str::<PluginManifest>(&text).expect("parse manifest")
        })
        .filter_map(|manifest| rd_plugin_host::provider_spec_from_manifest(&manifest))
        .collect();
    assert!(
        rows.len() >= 12,
        "expected the bundled provider rows, got {}",
        rows.len()
    );
    let rejected = rd_provider_registry::replace_dynamic(rows);
    assert!(rejected.is_empty(), "rejected rows: {rejected:?}");
}

#[test]
fn each_plugin_claims_one_provider_and_reaches_only_its_api() {
    register_bundled_providers();
    // One plugin, one provider: which plugin signs an account in is decided by this claim, and
    // a plugin claiming two would be a plugin nobody could update or switch off separately.
    for (source, provider) in [
        (DEBRIDLINK, "debridlink"),
        (ALLDEBRID, "alldebrid"),
        (PREMIUMIZE, "premiumize"),
    ] {
        let manifest = manifest(source);
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec![provider.to_owned()]);
        assert!(
            rd_provider_registry::by_slug(provider).is_some(),
            "{provider} must be a provider this build knows"
        );
        // Nothing beyond the provider's own API: no cookies, no captcha, no raw sockets, and
        // no vault reference — the credential it produces is written through the host.
        assert!(!manifest.capabilities.domains().is_empty());
        assert!(manifest.capabilities.net_stream.is_none());
        assert!(!manifest.capabilities.cookies);
        assert!(!manifest.capabilities.captcha);
        assert!(manifest.capabilities.secrets.is_empty());
    }
}

#[test]
fn every_declared_domain_belongs_to_the_provider_being_signed_in() {
    // The manifest's domain list is also the allowlist for the sign-in address a person is
    // asked to visit. A stray entry there would widen where that address may point, which is
    // the one thing this plugin type must not allow.
    for (source, expected) in [
        (DEBRIDLINK, vec!["debrid-link.com", "api.debrid-link.com"]),
        (ALLDEBRID, vec!["api.alldebrid.com"]),
        (PREMIUMIZE, vec!["www.premiumize.me", "premiumize.me"]),
    ] {
        assert_eq!(manifest(source).capabilities.domains(), expected.as_slice());
    }
}

//! What the bundled manifests have to keep delivering now that nothing is compiled in.
//!
//! Until RD-101-13 eleven provider rows were part of the binary, and `rd-provider-registry`
//! asserted these facts directly. That table is gone: a provider exists exactly when its plugin
//! is installed. The assertions did not stop mattering, they only changed owner — a hoster's
//! domains, its credential's hosts and its cookie scope now live in `plugins/*/manifest.toml`,
//! and a manifest edit that quietly drops one has to fail here.
//!
//! They also cover the gap the migration opened. While the built-in row existed, the manifest's
//! `[provider]` section was refused as a duplicate and therefore never exercised — so nothing
//! had ever checked that these halves were equivalent.

use std::path::PathBuf;

use url::Url;

fn plugin_manifests() -> Vec<rd_plugin_host::PluginManifest> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .canonicalize()
        .expect("plugins directory");
    std::fs::read_dir(root)
        .expect("read plugins")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.toml").is_file())
        .map(|path| {
            let text = std::fs::read_to_string(path.join("manifest.toml")).expect("manifest");
            toml::from_str(&text).expect("parse manifest")
        })
        .collect()
}

/// Registers every bundled provider row, the way startup does from the installed manifests.
fn register_bundled() {
    let rows: Vec<_> = plugin_manifests()
        .iter()
        .filter_map(rd_plugin_host::provider_spec_from_manifest)
        .collect();
    assert!(rows.len() >= 12, "expected the bundled provider rows");
    let rejected = rd_provider_registry::replace_dynamic(rows);
    assert!(rejected.is_empty(), "rejected rows: {rejected:?}");
}

fn url(value: &str) -> Url {
    value.parse().expect("url")
}

#[test]
fn nothing_is_a_provider_until_its_plugin_is_installed() {
    // Deliberately before `register_bundled`: this is the state of a fresh installation, and
    // it is what the accounts list now reflects instead of offering unresolvable hosters.
    assert!(rd_provider_registry::all().is_empty());
    assert!(rd_provider_registry::by_slug("ddownload").is_none());
    assert!(!rd_provider_registry::request_domain_allowed(
        "ddownload.com"
    ));

    register_bundled();
    assert_eq!(
        rd_provider_registry::by_slug("ddownload")
            .expect("installed")
            .slug,
        "ddownload"
    );
}

#[test]
fn every_bundled_provider_row_is_accepted() {
    register_bundled();
    // Every one of these was refused before the built-in table went, so none had ever been
    // exercised. The eleven former built-ins plus the plugin-only generic resolver, and the
    // two brands of RD-103-10/11.
    for slug in [
        "ddownload",
        "premiumize",
        "rapidgator",
        "nitroflare",
        "katfile",
        "1fichier",
        "keep2share",
        "filejoker",
        "alldebrid",
        "debridlink",
        "linksnappy",
        "xfs_generic",
        "krakenfiles",
        "turbobit",
        "hitfile",
        "mediafire",
    ] {
        let spec = rd_provider_registry::by_slug(slug)
            .unwrap_or_else(|| panic!("{slug} contributes no provider row"));
        assert_eq!(spec.source, rd_provider_registry::ProviderSource::Plugin);
    }
}

#[test]
fn url_mapping_and_aliases_survive_the_move_to_manifests() {
    register_bundled();
    for (address, slug) in [
        ("https://ddownload.com/abc123xyz", "ddownload"),
        ("https://www.ddownload.com/abc123xyz", "ddownload"),
        ("https://ddl.to/abc123xyz", "ddownload"),
        ("https://rapidgator.net/file/abc", "rapidgator"),
        ("https://www.rapidgator.net/file/abc", "rapidgator"),
        ("https://rg.to/file/abc", "rapidgator"),
        ("https://k2s.cc/file/abc", "keep2share"),
        ("https://nitroflare.com/view/abc", "nitroflare"),
        ("https://filejoker.net/abc", "filejoker"),
        ("https://1fichier.com/?abc", "1fichier"),
        ("https://tenvoi.com/?abc", "1fichier"),
        (
            "https://krakenfiles.com/view/DP3nGKJNsX/file.html",
            "krakenfiles",
        ),
        (
            "https://www.krakenfiles.com/embed-video/DP3nGKJNsX",
            "krakenfiles",
        ),
        (
            "https://www.mediafire.com/file/ipnyzofjcwri357",
            "mediafire",
        ),
        ("https://mfi.re/?ipnyzofjcwri357", "mediafire"),
        ("https://app.mediafire.com/ipnyzofjcwri357", "mediafire"),
    ] {
        assert_eq!(
            rd_provider_registry::provider_for_url(&url(address))
                .unwrap_or_else(|| panic!("{address} maps to no provider"))
                .slug,
            slug,
            "{address}"
        );
    }
    assert!(rd_provider_registry::provider_for_url(&url("https://example.com/file")).is_none());
}

/// katfile.biz is the canonical domain (JDownloader's `KatfileCom.getPluginDomains()` index 0);
/// the other six are aliases the site has used since, all rewriting to the same provider.
#[test]
fn katfile_keeps_its_canonical_domain_and_every_alias() {
    register_bundled();
    for host in [
        "katfile.biz",
        "katfile.space",
        "katfile.ws",
        "katfile.vip",
        "katfile.online",
        "katfile.cloud",
        "katfile.com",
    ] {
        assert_eq!(
            rd_provider_registry::provider_for_url(&url(&format!("https://{host}/abc123xyz")))
                .unwrap_or_else(|| panic!("{host} should resolve to katfile"))
                .slug,
            "katfile",
            "{host}"
        );
    }
    assert!(rd_provider_registry::request_domain_allowed("katfile.biz"));
    assert!(rd_provider_registry::request_domain_allowed(
        "fs7.katfile.biz"
    ));
    assert!(rd_provider_registry::secret_domain_allowed(
        "katfile_api_key",
        &url("https://katfile.biz/api/account/info")
    ));
    // An alias domain is not where the credential may go.
    assert!(!rd_provider_registry::secret_domain_allowed(
        "katfile_api_key",
        &url("https://katfile.com/api/account/info")
    ));
    assert_eq!(
        rd_provider_registry::cookie_scope("katfile")
            .expect("scope")
            .as_str(),
        "https://katfile.biz/"
    );
}

/// DDownload's own hosts plus the CDN it delivers from.
///
/// The CDN is reached by the account-less flow as a request of its own, so this gate has to
/// allow it — while the API key stays pinned to `api-v2.ddownload.com` through its own slot.
#[test]
fn ddownload_request_domains_cover_its_hosts_and_its_delivery_cdn() {
    register_bundled();
    assert!(rd_provider_registry::request_domain_allowed(
        "ddownload.com"
    ));
    assert!(rd_provider_registry::request_domain_allowed(
        "api-v2.ddownload.com"
    ));
    assert!(rd_provider_registry::request_domain_allowed(
        "eu-hydra5.zeuscdn.org"
    ));
    assert!(!rd_provider_registry::request_domain_allowed("evil.com"));
    assert!(!rd_provider_registry::secret_domain_allowed(
        "ddownload_api_key",
        &url("https://eu-hydra5.zeuscdn.org/d/token/release.rar")
    ));
}

/// The two credential modes stay on their own hosts: the key only to the API, the password only
/// to the website's login form. This is the separation the whole slot mechanism exists for.
#[test]
fn credentials_stay_on_the_hosts_their_slot_names() {
    register_bundled();
    assert!(rd_provider_registry::secret_reference_allowed(
        "ddownload",
        "ddownload_api_key"
    ));
    assert!(rd_provider_registry::secret_reference_allowed(
        "ddownload",
        "ddownload_password"
    ));
    assert!(!rd_provider_registry::secret_reference_allowed(
        "ddownload",
        "premiumize_api_key"
    ));
    assert!(!rd_provider_registry::secret_reference_allowed(
        "ddownload",
        "unknown"
    ));

    assert!(rd_provider_registry::secret_domain_allowed(
        "ddownload_api_key",
        &url("https://api-v2.ddownload.com/api/account/info")
    ));
    assert!(!rd_provider_registry::secret_domain_allowed(
        "ddownload_api_key",
        &url("https://ddownload.com/")
    ));
    assert!(!rd_provider_registry::secret_domain_allowed(
        "ddownload_api_key",
        &url("https://evil.com/")
    ));

    assert!(rd_provider_registry::secret_reference_allowed(
        "premiumize",
        "premiumize_api_key"
    ));
    assert!(!rd_provider_registry::secret_reference_allowed(
        "premiumize",
        "ddownload_api_key"
    ));
    assert!(rd_provider_registry::secret_domain_allowed(
        "premiumize_api_key",
        &url("https://www.premiumize.me/api/account/info")
    ));
    assert!(!rd_provider_registry::secret_domain_allowed(
        "premiumize_api_key",
        &url("https://premiumize.me/")
    ));
}

#[test]
fn cookie_scopes_survive_the_move_to_manifests() {
    register_bundled();
    assert_eq!(
        rd_provider_registry::cookie_scope("ddownload")
            .expect("scope")
            .as_str(),
        "https://ddownload.com/"
    );
    // Premiumize deliberately has none, unlike the built-in row it replaced. That row declared a
    // scope the plugin could never use: it is an API-key multihoster with no `cookies`
    // capability, and the manifest check refuses a grant the runtime does not enforce. Without a
    // scope the host falls back to the request host, which for a plugin that touches no cookies
    // decides nothing.
    assert!(rd_provider_registry::cookie_scope("premiumize").is_none());
    assert!(rd_provider_registry::cookie_scope("unknown").is_none());
}

#[test]
fn premiumize_keeps_the_apex_domain_its_builtin_row_granted() {
    register_bundled();
    assert!(rd_provider_registry::request_domain_allowed(
        "premiumize.me"
    ));
    assert!(rd_provider_registry::request_domain_allowed(
        "www.premiumize.me"
    ));
    assert!(!rd_provider_registry::request_domain_allowed("evil.com"));
}

/// Turbobit and HitFile (RD-103-10, RD-103-11): every live short domain rewrites to the main
/// site, the API host is requestable, and the password reaches the API host and nothing else —
/// not the site the links point at, not a short domain.
#[test]
fn turbobit_and_hitfile_keep_their_short_domains_and_pin_the_password_to_the_api() {
    register_bundled();
    for (host, slug) in [
        ("turbobit.net", "turbobit"),
        ("www.turbobit.net", "turbobit"),
        ("new.turbobit.net", "turbobit"),
        ("m.turbobit.net", "turbobit"),
        ("turbobit.cc", "turbobit"),
        ("turb.cc", "turbobit"),
        ("turb.pw", "turbobit"),
        ("turbo.to", "turbobit"),
        ("trbt.cc", "turbobit"),
        ("hitfile.net", "hitfile"),
        ("www.hitfile.net", "hitfile"),
        ("new.hitfile.net", "hitfile"),
        ("hitfile.ru", "hitfile"),
        ("hil.to", "hitfile"),
        ("hitf.cc", "hitfile"),
        ("htfl.net", "hitfile"),
        ("hitf.to", "hitfile"),
    ] {
        assert_eq!(
            rd_provider_registry::provider_for_url(&url(&format!(
                "https://{host}/a1b2c3d4e5f6.html"
            )))
            .unwrap_or_else(|| panic!("{host} should resolve to {slug}"))
            .slug,
            slug,
            "{host}"
        );
    }
    for (slug, reference, api, site) in [
        (
            "turbobit",
            "turbobit_password",
            "app.turbobit.net",
            "turbobit.net",
        ),
        (
            "hitfile",
            "hitfile_password",
            "app.hitfile.net",
            "hitfile.net",
        ),
    ] {
        assert!(rd_provider_registry::request_domain_allowed(api));
        assert!(rd_provider_registry::request_domain_allowed(site));
        assert!(rd_provider_registry::secret_reference_allowed(
            slug, reference
        ));
        assert!(rd_provider_registry::secret_domain_allowed(
            reference,
            &url(&format!("https://{api}/api/auth/login"))
        ));
        assert!(!rd_provider_registry::secret_domain_allowed(
            reference,
            &url(&format!("https://{site}/login"))
        ));
        // No cookie scope: the plugin declares no cookies capability, and the session lives in
        // the host's own jar.
        assert!(rd_provider_registry::cookie_scope(slug).is_none());
    }
    assert!(!rd_provider_registry::secret_domain_allowed(
        "turbobit_password",
        &url("https://app.hitfile.net/api/auth/login")
    ));
    assert!(!rd_provider_registry::request_domain_allowed("turb.pw"));
}

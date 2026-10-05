use std::sync::Arc;

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    AccountStatus, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedDownload, Resolver, ResolverHost, ResolverMetadata,
};

use super::compatible_components;
use crate::{PluginManifest, VerifiedPackage};

const MULTIHOSTER: &str = "019d0000-0000-7000-8000-000000001801";
const FREE_HOSTER: &str = "019d0000-0000-7000-8000-000000001802";

struct UnusedHost;

#[async_trait]
impl ResolverHost for UnusedHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        _request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        Err(Failure::new(
            rd_core::FailureKind::Permanent,
            "unexpected HTTP request",
        ))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

#[test]
fn incompatible_installed_component_is_skipped() {
    let manifest: PluginManifest = toml::from_str(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-0000000000ff"
name = "Outdated"
version = "0.1.0"
key_id = "fixture"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "Outdated fixture"
author = "Fixture Author"

[provider]
slug = "outdated"
kind = "hoster"
credentials = "api_key"
"#,
    )
    .expect("manifest");
    let package = VerifiedPackage {
        manifest,
        manifest_bytes: Vec::new(),
        component: b"\0asm\x0d\0\x01\0".to_vec(),
        signature: None,
        locales: Vec::new(),
    };

    let loaded = compatible_components(
        &crate::PluginTypeRegistry::new(vec![package]),
        Arc::new(UnusedHost),
        crate::ExecutionLog::disabled(),
    );

    assert!(loaded.is_empty());
}

/// A hoster link that no resolver can serve must surface an error. Reporting "resolved
/// to nothing" instead made the scheduler download the hoster's landing page and store
/// it under the link's name as a finished download.
#[test]
fn an_unresolvable_hoster_link_is_reported_as_needing_an_account() {
    super::register_bundled_providers_for_tests();
    let url = "https://rapidgator.net/file/abc123/archive.rar"
        .parse()
        .expect("URL");

    let failure = super::account_required(&url).expect("hoster link must fail");

    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("resolve.account_required"));
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("rapidgator.net")
    );
    assert_eq!(
        failure.params.get("provider").map(String::as_str),
        Some("rapidgator")
    );
}

/// Plain HTTP links are still downloaded exactly as they were added.
///
/// A multihoster declares a `*` domain, so asking the resolvers whether one "matches"
/// a URL answers yes for every link on the internet. Classifying on that basis made
/// every direct download fail with "this hoster needs an account"; the registry is
/// consulted instead, and it claims a URL only for a hoster that lists its host.
#[test]
fn a_direct_link_is_not_mistaken_for_a_hoster_link() {
    for direct in [
        "https://cdn.example.test/releases/tool.bin",
        "http://127.0.0.1:8080/payload.bin",
        "https://files.example.org/a/b/c.zip",
    ] {
        let url: url::Url = direct.parse().expect("URL");
        assert!(
            super::account_required(&url).is_none(),
            "{direct} must stay a direct download"
        );
    }
}

/// A resolver that claims hosts by name, or every host like a multihoster's `*`.
struct Claims {
    metadata: ResolverMetadata,
    hosts: Vec<&'static str>,
}

impl Claims {
    fn resolver(
        plugin_id: &str,
        slug: &str,
        hosts: Vec<&'static str>,
        requires_account: bool,
    ) -> Arc<dyn Resolver> {
        Arc::new(Self {
            metadata: ResolverMetadata {
                plugin_id: plugin_id.parse().expect("plugin id"),
                name: slug.to_owned(),
                version: "1.0.0".to_owned(),
                provider_slug: slug.to_owned(),
                domains: hosts.iter().map(|host| (*host).to_owned()).collect(),
                max_concurrent_downloads: 1,
                requires_account,
            },
            hosts,
        })
    }
}

#[async_trait]
impl Resolver for Claims {
    fn metadata(&self) -> &ResolverMetadata {
        &self.metadata
    }

    fn matches(&self, url: &url::Url) -> bool {
        url.host_str().is_some_and(|host| {
            self.hosts
                .iter()
                .any(|claimed| *claimed == "*" || *claimed == host)
        })
    }

    async fn check_account(&self, _account_id: AccountId) -> Result<AccountStatus, Failure> {
        Err(Failure::new(rd_core::FailureKind::Permanent, "unused"))
    }

    async fn resolve(&self, _request: ResolveRequest) -> Result<ResolvedDownload, Failure> {
        Err(Failure::new(
            rd_core::FailureKind::Permanent,
            "resolved by the stub",
        ))
    }
}

async fn service(directory: &std::path::Path) -> super::ResolverService {
    let database = rd_db::Database::open(directory.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secret store");
    super::ResolverService::new(
        database,
        rd_http::ClientPool::default(),
        secrets,
        Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
        None,
        crate::OwnEndpoints::default(),
    )
}

/// Nothing is compiled in (RD-150-18): until the installed components are loaded the chain
/// is empty, and a hoster link fails with "account required" instead of reaching a
/// resolver nobody installed.
#[tokio::test]
async fn a_fresh_service_has_no_resolver_until_components_are_loaded() {
    super::register_bundled_providers_for_tests();
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    let hoster: url::Url = "https://rapidgator.net/file/abc/x.rar"
        .parse()
        .expect("URL");

    assert!(service.chain().resolvers.is_empty());
    assert!(!service.has_resolver(&hoster));
    assert!(!service.has_free_resolver(&hoster));
    let failure = service
        .resolve(hoster, None, None, None)
        .await
        .expect_err("a hoster link needs a resolver");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
}

/// The end of the wiring an account-less download depends on: a hoster whose resolver
/// opted into free downloads must be found without an account, and a plain HTTP link
/// must not be claimed by one. Without this, `resolve()` reports "account required" and
/// the free flow is unreachable no matter how complete the plugin is.
#[tokio::test]
async fn a_free_capable_hoster_is_found_without_an_account() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    service.replace_chain(super::Chain {
        resolvers: vec![
            Claims::resolver(MULTIHOSTER, "premiumize", vec!["*"], true),
            Claims::resolver(FREE_HOSTER, "katfile", vec!["katfile.biz"], false),
        ],
        pin_only: std::collections::HashSet::new(),
    });

    let katfile: url::Url = "https://katfile.biz/abc123xyz/release.rar"
        .parse()
        .expect("URL");
    assert_eq!(
        service.free_resolver_plugin(&katfile),
        Some(FREE_HOSTER.parse::<rd_core::PluginId>().expect("plugin id")),
        "a resolver with requires_account = false must be reachable without an account"
    );

    let direct: url::Url = "https://cdn.example.test/tool.bin".parse().expect("URL");
    assert!(
        !service.has_free_resolver(&direct),
        "a plain HTTP link must stay a direct download"
    );
}

/// `resolve()` against a chain with a multihoster whose `*` domain matches every URL. A
/// plain link must come back as "not resolved" so the scheduler downloads it directly;
/// anything else breaks every direct download, which a chain without such a resolver
/// cannot show.
#[tokio::test]
async fn a_plain_link_resolves_to_nothing_even_though_multihosters_match_every_host() {
    super::register_bundled_providers_for_tests();
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    service.replace_chain(super::Chain {
        resolvers: vec![Claims::resolver(MULTIHOSTER, "premiumize", vec!["*"], true)],
        pin_only: std::collections::HashSet::new(),
    });

    for direct in [
        "http://127.0.0.1:8792/payload.bin",
        "https://cdn.example.test/releases/tool.bin",
    ] {
        let url: url::Url = direct.parse().expect("URL");
        let resolved = service
            .resolve(url, None, None, None)
            .await
            .unwrap_or_else(|failure| panic!("{direct} must not fail: {failure}"));
        assert!(
            resolved.is_none(),
            "{direct} must fall through to a direct download"
        );
    }

    // A known hoster with no account still fails loudly rather than downloading its page.
    let hoster: url::Url = "https://rapidgator.net/file/abc/x.rar"
        .parse()
        .expect("URL");
    let failure = service
        .resolve(hoster, None, None, None)
        .await
        .expect_err("a hoster link needs a resolver");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
}

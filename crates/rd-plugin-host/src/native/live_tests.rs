//! A plugin installed while the service runs (RD-170-12): a first install joins the running
//! chain, an update of a loaded plugin waits for the next start, and an account whose plugin
//! waits hears that rather than "no resolver is installed".

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{AccountStatus, ResolveRequest, ResolvedDownload, Resolver, ResolverMetadata};

use super::{Chain, ResolverService};
use crate::{PluginManifest, PluginTypeRegistry, VerifiedPackage};

/// DDownload's plugin id, the plugin of the report.
const DDOWNLOAD: &str = "019d0000-0000-7000-8000-000000000001";

async fn service(directory: &std::path::Path) -> ResolverService {
    let database = rd_db::Database::open(directory.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secret store");
    ResolverService::new(
        database,
        rd_http::ClientPool::default(),
        secrets,
        Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
        None,
    )
}

/// The shipped DDownload package with its built component, as the installer would verify it.
fn ddownload_registry() -> PluginTypeRegistry {
    let manifest_bytes =
        std::fs::read("../../plugins/ddownload/manifest.toml").expect("the ddownload manifest");
    let manifest: PluginManifest =
        toml::from_str(std::str::from_utf8(&manifest_bytes).expect("utf-8")).expect("manifest");
    PluginTypeRegistry::new(vec![VerifiedPackage {
        manifest,
        manifest_bytes,
        component: crate::artifact::component("rd-plugin-ddownload"),
        signature: None,
        locales: Vec::new(),
    }])
}

/// A stand-in for a DDownload version this start already loaded.
struct Loaded(ResolverMetadata);

#[async_trait]
impl Resolver for Loaded {
    fn metadata(&self) -> &ResolverMetadata {
        &self.0
    }

    fn matches(&self, url: &url::Url) -> bool {
        url.host_str() == Some("ddownload.com")
    }

    async fn check_account(&self, _account_id: AccountId) -> Result<AccountStatus, Failure> {
        Err(Failure::new(rd_core::FailureKind::Permanent, "unused"))
    }

    async fn resolve(&self, _request: ResolveRequest) -> Result<ResolvedDownload, Failure> {
        Err(Failure::new(rd_core::FailureKind::Permanent, "unused"))
    }
}

fn loaded(version: &str) -> Arc<dyn Resolver> {
    Arc::new(Loaded(ResolverMetadata {
        plugin_id: DDOWNLOAD.parse().expect("plugin id"),
        name: "DDownload".to_owned(),
        version: version.to_owned(),
        provider_slug: "ddownload".to_owned(),
        domains: vec!["ddownload.com".to_owned()],
        max_concurrent_downloads: 1,
        requires_account: true,
    }))
}

/// The owner's report: DDownload installed from the wizard, and the account check right after
/// it found no resolver, because the chain was built at the start. A first install now joins
/// the chain the running service already has — the same service, no new host.
#[tokio::test]
async fn a_first_install_joins_the_running_chain_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    let reader = service.clone();
    let plugin = DDOWNLOAD.parse().expect("plugin id");
    let url: url::Url = "https://ddownload.com/abc123xyz/release.rar"
        .parse()
        .expect("URL");
    assert!(!reader.has_plugin(plugin));
    assert!(!reader.has_resolver(&url));

    assert_eq!(service.activate_first_install(&ddownload_registry()), 1);

    // A clone taken before the install sees it: every holder shares one chain.
    assert!(reader.has_plugin(plugin));
    assert!(reader.has_resolver(&url));
    let resolver = reader
        .resolver_for_provider("ddownload", None)
        .expect("lookup")
        .expect("the provider has its resolver now");
    assert_eq!(resolver.metadata().plugin_id, plugin);

    // Installing it again adds nothing: it is loaded now.
    assert_eq!(service.activate_first_install(&ddownload_registry()), 0);
    assert_eq!(reader.chain().resolvers.len(), 1);
}

/// A download that started on one version must not meet another half-way, so a plugin with
/// any version in the chain keeps it until the next start.
#[tokio::test]
async fn an_update_of_a_loaded_plugin_does_not_switch_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    service.replace_chain(Chain {
        resolvers: vec![loaded("0.0.1")],
        pin_only: HashSet::new(),
    });

    assert_eq!(service.activate_first_install(&ddownload_registry()), 0);

    let chain = service.chain();
    assert_eq!(chain.resolvers.len(), 1);
    let resolver = service
        .resolver_for_provider("ddownload", None)
        .expect("lookup")
        .expect("the loaded version");
    assert_eq!(resolver.metadata().version, "0.0.1");
}

/// An account whose plugin is installed but not running is told it runs after a restart; one
/// whose provider has no waiting plugin still hears that no resolver is installed.
#[tokio::test]
async fn an_account_of_a_waiting_plugin_hears_it_runs_after_a_restart() {
    super::register_bundled_providers_for_tests();
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    let account = service
        .database
        .create_account(rd_db::NewAccount {
            provider: "ddownload".to_owned(),
            label: "DDownload".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let failure = service
        .check_account(account.id)
        .await
        .expect_err("nothing is loaded");
    assert_eq!(failure.code.as_deref(), Some("plugin.resolver_missing"));

    service.mark_waiting(DDOWNLOAD.parse().expect("plugin id"));
    for failure in [
        service.check_account(account.id).await.err(),
        service.hosters(account.id).await.err(),
        service
            .check(
                account.id,
                vec!["https://ddownload.com/abc123xyz".parse().expect("URL")],
            )
            .await
            .err(),
    ] {
        let failure = failure.expect("the plugin waits for a restart");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.installed_not_running")
        );
    }
}

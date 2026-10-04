//! The resolver chain under a version choice (RD-140-02): new work meets only the default
//! version, and a staged or retained version is reached through an explicit pin alone.

use std::sync::Arc;

use async_trait::async_trait;
use rd_core::{AccountId, Failure, ResolverPin};
use rd_plugin_api::{AccountStatus, ResolveRequest, ResolvedDownload, Resolver, ResolverMetadata};
use url::Url;

use super::{Chain, ResolverService};

const PLUGIN: &str = "019d0000-0000-7000-8000-000000001402";

/// One version of one free resolver; the address it answers with names that version.
struct Versioned {
    metadata: ResolverMetadata,
    /// Hosts this version claims. A newer version claiming more is the case that matters.
    hosts: Vec<&'static str>,
}

impl Versioned {
    fn resolver(version: &str, hosts: Vec<&'static str>) -> Arc<dyn Resolver> {
        Arc::new(Self {
            metadata: ResolverMetadata {
                plugin_id: PLUGIN.parse().expect("plugin id"),
                name: "Versioned".to_owned(),
                version: version.to_owned(),
                provider_slug: "versioned".to_owned(),
                domains: hosts.iter().map(|host| (*host).to_owned()).collect(),
                max_concurrent_downloads: 1,
                requires_account: false,
            },
            hosts,
        })
    }
}

#[async_trait]
impl Resolver for Versioned {
    fn metadata(&self) -> &ResolverMetadata {
        &self.metadata
    }

    fn matches(&self, url: &Url) -> bool {
        url.host_str()
            .is_some_and(|host| self.hosts.contains(&host))
    }

    async fn check_account(&self, _account_id: AccountId) -> Result<AccountStatus, Failure> {
        Err(Failure::new(rd_core::FailureKind::Permanent, "unused"))
    }

    async fn resolve(&self, request: ResolveRequest) -> Result<ResolvedDownload, Failure> {
        Ok(ResolvedDownload {
            url: format!("https://cdn.example.test/{}", self.metadata.version)
                .parse()
                .expect("URL"),
            file_name: None,
            size: None,
            headers: Vec::new(),
            checksum: None,
            client: request.client,
        })
    }
}

async fn service(directory: &std::path::Path) -> ResolverService {
    let database = rd_db::Database::open(directory.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secret store");
    let service = ResolverService::new(
        database,
        rd_http::ClientPool::default(),
        secrets,
        Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
        None,
        crate::OwnEndpoints::default(),
    );
    // 1.0.0 is the default (a rollback, say), 2.0.0 is under test and claims one more host.
    service.replace_chain(Chain {
        resolvers: vec![
            Versioned::resolver("1.0.0", vec!["old.example.test"]),
            Versioned::resolver("2.0.0", vec!["old.example.test", "new.example.test"]),
        ],
        pin_only: [(PLUGIN.parse().expect("plugin id"), "2.0.0".to_owned())]
            .into_iter()
            .collect(),
    });
    service
}

async fn resolved_version(
    service: &ResolverService,
    url: &str,
    pin: Option<&ResolverPin>,
) -> Option<String> {
    service
        .resolve(url.parse().expect("URL"), None, None, pin)
        .await
        .expect("resolve")
        .map(|resolved| resolved.url.path().trim_start_matches('/').to_owned())
}

#[tokio::test]
async fn an_unpinned_job_runs_on_the_default_version_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;

    assert_eq!(
        resolved_version(&service, "https://old.example.test/f", None).await,
        Some("1.0.0".to_owned())
    );
    // The staged version claims this host and the default does not: an unpinned job must not
    // fall through to the version under test.
    assert_eq!(
        resolved_version(&service, "https://new.example.test/f", None).await,
        None
    );
    assert!(
        !service.has_resolver(&"https://new.example.test/f".parse().expect("URL")),
        "intake must not accept an address only the staged version claims"
    );
    assert!(
        service
            .free_resolver_plugin(&"https://new.example.test/f".parse().expect("URL"))
            .is_none()
    );
}

#[tokio::test]
async fn a_job_pinned_to_the_staged_version_runs_on_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let service = service(directory.path()).await;
    let staged = ResolverPin {
        plugin_id: PLUGIN.parse().expect("plugin id"),
        version: "2.0.0".to_owned(),
    };

    assert_eq!(
        resolved_version(&service, "https://old.example.test/f", Some(&staged)).await,
        Some("2.0.0".to_owned())
    );
    assert_eq!(
        resolved_version(&service, "https://new.example.test/f", Some(&staged)).await,
        Some("2.0.0".to_owned())
    );
}

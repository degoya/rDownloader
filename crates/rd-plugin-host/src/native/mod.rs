use std::{
    collections::HashSet,
    sync::{Arc, PoisonError, RwLock},
};

use rd_core::{AccountId, Failure, FailureKind, LinkCheckResult, ProxyProfileId, ResolverPin};
use rd_http::{ClientPool, SharedNetworkDefaults};
use rd_plugin_api::{
    AccountStatus, CheckRequest, ClientIdentity, ResolveRequest, ResolvedDownload, Resolver,
    ResolverHost,
};
use url::Url;

#[cfg(test)]
mod bundled_headers_tests;
mod expand;
pub(crate) mod granted;
mod host;
#[cfg(test)]
mod live_tests;
mod references;
mod signin;
mod transfer_auth;
#[cfg(test)]
mod versions_tests;

pub use expand::{
    CLIENT_ID_MARKER, client_not_configured, provider_cookie_scope, provider_download_bearer,
    provider_token_beside_the_flow,
};
pub(crate) use granted::GrantedHost;
use host::NativeHost;
pub(crate) use host::{with_own_network, with_response_allowance};
pub use transfer_auth::{provider_download_authorization, provider_download_carries_credential};

/// The resolver chain: the installed resolver components, on the application's own host.
#[derive(Clone)]
pub struct ResolverService {
    database: rd_db::Database,
    /// Replaced whole, never edited in place: a lookup takes the chain as it is at that moment
    /// and keeps it for the rest of the call, so a first install joining it (RD-170-12) never
    /// blocks a running download or shows it half a chain. Shared by every clone.
    chain: Arc<RwLock<Arc<Chain>>>,
    /// Resolver plugins installed while this start runs that are not in the chain: an update of
    /// a loaded plugin, or a first install that could not be loaded (RD-170-12). An account of
    /// their provider is told the plugin runs after a restart, not that there is none.
    waiting: Arc<RwLock<HashSet<rd_core::PluginId>>>,
    host: Arc<dyn ResolverHost>,
}

/// The loaded resolvers, and which of them only a pinned job may use.
#[derive(Default)]
struct Chain {
    resolvers: Vec<Arc<dyn Resolver>>,
    /// `(plugin id, version)` of every loaded resolver that is not its plugin's default:
    /// retained versions and a staged one (RD-140-02). They serve a download pinned to them
    /// and are skipped by every unpinned lookup, so new work only ever meets the default.
    pin_only: HashSet<(rd_core::PluginId, String)>,
}

impl Chain {
    /// Whether an unpinned lookup may pick this resolver: only its plugin's default version.
    fn selectable(&self, resolver: &Arc<dyn Resolver>) -> bool {
        let metadata = resolver.metadata();
        !self
            .pin_only
            .contains(&(metadata.plugin_id, metadata.version.clone()))
    }

    /// Whether `resolver` is the one a job with `pin` may use: exactly the pinned version, or
    /// without a pin the plugin's default.
    fn admits(&self, resolver: &Arc<dyn Resolver>, pin: Option<&ResolverPin>) -> bool {
        match pin {
            Some(pin) => {
                let metadata = resolver.metadata();
                metadata.plugin_id == pin.plugin_id && metadata.version == pin.version
            }
            None => self.selectable(resolver),
        }
    }
}

impl ResolverService {
    /// The service, with `own` as the listeners no plugin request may reach (RA-HOST-01). They
    /// are also published for the decisions made without a host in hand, such as whether a
    /// notification target may be saved.
    #[must_use]
    pub fn new(
        database: rd_db::Database,
        clients: ClientPool,
        secrets: rd_secrets::SecretStore,
        network_defaults: SharedNetworkDefaults,
        captcha: Option<Arc<dyn rd_plugin_api::CaptchaSolver>>,
        own: crate::OwnEndpoints,
    ) -> Self {
        crate::own_endpoints::publish(&own);
        let host: Arc<dyn ResolverHost> = Arc::new(
            NativeHost::new(
                database.clone(),
                clients,
                secrets,
                network_defaults,
                captcha,
            )
            .with_own_endpoints(own),
        );
        Self {
            database,
            // Nothing is compiled in (RD-150-18): the chain is the installed resolver
            // components, and it is empty until `load_components_from_registry` fills it.
            chain: Arc::default(),
            waiting: Arc::default(),
            host,
        }
    }

    /// The application's own host capabilities, unnarrowed.
    ///
    /// Handed to the extension plugin types, which narrow it to their own manifest exactly as
    /// a resolver does. Nothing here is a grant by itself — every call through it is still
    /// checked against the manifest of whoever made it.
    #[must_use]
    pub fn host(&self) -> Arc<dyn ResolverHost> {
        Arc::clone(&self.host)
    }

    /// The chain as it is now; the lock is held only for the copy of one pointer.
    fn chain(&self) -> Arc<Chain> {
        Arc::clone(&*self.chain.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn replace_chain(&self, chain: Chain) {
        *self.chain.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(chain);
    }

    /// Loads the installed Components from a registry the adapters share, with each resolver
    /// pinning one exact version.
    ///
    /// Once the final resolver chain is known, jobs pinned to a version that is no longer in
    /// it are released. Anything else leaves those jobs failing with
    /// `plugin.pinned_version_missing` on every retry for the rest of their life.
    ///
    /// Building a registry re-verifies *every* installed package — an Ed25519 check and a
    /// wasmparser validation each, and a compile or a compile-cache read for any content the
    /// process has not compiled yet — so the resolvers take the shared one rather than a
    /// private pass; a service that starts them all pays for one.
    pub async fn load_components_from_registry(
        &mut self,
        registry: &crate::PluginTypeRegistry,
    ) -> anyhow::Result<usize> {
        let components = compatible_components(
            registry,
            Arc::clone(&self.host),
            crate::ExecutionLog::new(self.database.clone()),
        );
        let count = components.len();
        let mut pin_only = HashSet::new();
        let mut loaded = Vec::with_capacity(count);
        for (resolver, role) in components {
            if role != crate::VersionRole::Default {
                let metadata = resolver.metadata();
                pin_only.insert((metadata.plugin_id, metadata.version.clone()));
            }
            loaded.push(resolver);
        }
        self.replace_chain(Chain {
            resolvers: loaded,
            pin_only,
        });
        self.release_unsatisfiable_pins().await;
        Ok(count)
    }

    /// Whether any version of this plugin is in the running chain.
    #[must_use]
    pub fn has_plugin(&self, id: rd_core::PluginId) -> bool {
        self.chain()
            .resolvers
            .iter()
            .any(|resolver| resolver.metadata().plugin_id == id)
    }

    /// Joins the resolvers of a plugin this start did not load to the running chain
    /// (RD-170-12), and returns how many joined.
    ///
    /// Only a first install: a plugin id that already has any version in the chain is left
    /// alone, because a download that started on one version must not meet another half-way
    /// — an update of a loaded plugin runs from the next start, as it always has. Only the
    /// default version of each package joins; the chain is swapped whole, so nothing that is
    /// resolving right now waits for this or sees a chain in between.
    ///
    /// Compiles the components, so it belongs on a blocking thread.
    pub fn activate_first_install(&self, registry: &crate::PluginTypeRegistry) -> usize {
        let built: Vec<Arc<dyn Resolver>> = compatible_components(
            registry,
            Arc::clone(&self.host),
            crate::ExecutionLog::new(self.database.clone()),
        )
        .into_iter()
        .filter(|(_, role)| *role == crate::VersionRole::Default)
        .map(|(resolver, _)| resolver)
        .collect();
        let mut current = self.chain.write().unwrap_or_else(PoisonError::into_inner);
        let loaded: HashSet<rd_core::PluginId> = current
            .resolvers
            .iter()
            .map(|resolver| resolver.metadata().plugin_id)
            .collect();
        let joining: Vec<Arc<dyn Resolver>> = built
            .into_iter()
            .filter(|resolver| !loaded.contains(&resolver.metadata().plugin_id))
            .collect();
        if joining.is_empty() {
            return 0;
        }
        let mut resolvers = current.resolvers.clone();
        resolvers.extend(joining.iter().cloned());
        let pin_only = current.pin_only.clone();
        *current = Arc::new(Chain {
            resolvers,
            pin_only,
        });
        drop(current);
        let mut waiting = self.waiting.write().unwrap_or_else(PoisonError::into_inner);
        for resolver in &joining {
            waiting.remove(&resolver.metadata().plugin_id);
        }
        joining.len()
    }

    /// Records a resolver plugin installed while this start runs that is not in the chain, so
    /// an account of its provider hears that it runs after a restart (RD-170-12).
    pub fn mark_waiting(&self, id: rd_core::PluginId) {
        self.waiting
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id);
    }

    /// Frees jobs whose pinned resolver version this build cannot provide any more.
    async fn release_unsatisfiable_pins(&self) {
        let available = self
            .chain()
            .resolvers
            .iter()
            .map(|resolver| {
                let metadata = resolver.metadata();
                (metadata.plugin_id.to_string(), metadata.version.clone())
            })
            .collect();
        match self
            .database
            .clear_unsatisfiable_resolver_pins(available)
            .await
        {
            Ok(0) => {}
            Ok(freed) => tracing::info!(
                freed,
                "released downloads pinned to a resolver version this build no longer has"
            ),
            // Diagnostics for stale pins must not keep the service from starting; the jobs
            // simply stay pinned and report the missing version as they did before.
            Err(error) => {
                tracing::warn!(error = %error, "could not release stale resolver pins")
            }
        }
    }

    /// Resolves with the explicitly selected provider account, or returns direct HTTP unchanged.
    /// Without an account, a resolver that declares `requires_account: false` may still resolve
    /// the link (free download); everything else falls through to direct HTTP.
    pub async fn resolve(
        &self,
        url: Url,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<ResolvedDownload>, Failure> {
        // Before any installed resolver sees the link: a marker inside it would
        // be expanded with the account's credential (RD-120-66).
        if crate::foreign_address::carries_marker(url.as_str()) {
            return Err(crate::foreign_address::refused());
        }
        let Some(account_id) = account_id else {
            let chain = self.chain();
            let resolver = chain.resolvers.iter().find(|resolver| {
                !resolver.metadata().requires_account
                    && chain.admits(resolver, pin)
                    && resolver.matches(&url)
            });
            let Some(resolver) = resolver.cloned() else {
                // A hoster link with no free path must fail visibly. Falling through to
                // direct HTTP would download the hoster's landing page and store it under
                // the link's name as a completed download.
                if let Some(failure) = account_required(&url) {
                    return Err(failure);
                }
                return Ok(None);
            };
            return resolver
                .resolve(ResolveRequest {
                    url,
                    client: ClientIdentity {
                        account_id: None,
                        proxy_profile_id,
                        tls_revision: 0,
                    },
                })
                .await
                .map(Some);
        };
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self
            .resolver_for_provider(&provider, pin)?
            .filter(|resolver| resolver.matches(&url));
        let Some(resolver) = resolver else {
            // The selected account cannot serve this link (a plain direct URL routed
            // through a multihoster account is the legitimate case); a hoster link still
            // must not degrade into a landing-page download.
            if let Some(failure) = account_required(&url) {
                return Err(failure);
            }
            return Ok(None);
        };
        resolver
            .resolve(ResolveRequest {
                url,
                client: ClientIdentity {
                    account_id: Some(account_id),
                    proxy_profile_id,
                    tls_revision: 0,
                },
            })
            .await
            .map(Some)
    }

    /// The plugin whose resolver can serve this link without an account, if any.
    ///
    /// Two callers need this: the link check, to explain a link it cannot verify but that
    /// will still download, and the scheduler, to serialise a hoster's free downloads.
    #[must_use]
    pub fn free_resolver_plugin(&self, url: &Url) -> Option<rd_core::PluginId> {
        let chain = self.chain();
        chain
            .resolvers
            .iter()
            .find(|resolver| {
                !resolver.metadata().requires_account
                    && chain.selectable(resolver)
                    && resolver.matches(url)
            })
            .map(|resolver| resolver.metadata().plugin_id)
    }

    /// Whether some installed resolver can serve this link without an account.
    #[must_use]
    pub fn has_free_resolver(&self, url: &Url) -> bool {
        self.free_resolver_plugin(url).is_some()
    }

    /// Whether any installed resolver speaks for this address at all -- free or account-bound.
    ///
    /// Broader than [`Self::has_free_resolver`] on purpose, and asked by exactly one caller:
    /// the verdict a crawled address passes before it may become a candidate (RD-110-07).
    /// There the question is not "can this be downloaded right now" but "is this a hoster
    /// link rather than an arbitrary page" -- an address a resolver claims is one the resolver
    /// turns into a file, so nothing is gained by fetching it here to look at its content
    /// type, and a HEAD against a hoster's landing page would answer `text/html` anyway.
    #[must_use]
    pub fn has_resolver(&self, url: &Url) -> bool {
        let chain = self.chain();
        chain
            .resolvers
            .iter()
            .any(|resolver| chain.selectable(resolver) && resolver.matches(url))
    }

    /// Runs the provider resolver's redaction-safe account check.
    pub async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self.provider_resolver(&provider)?;
        resolver.check_account(account_id).await
    }

    /// Hoster catalogue of the account's provider resolver.
    pub async fn hosters(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self.provider_resolver(&provider)?;
        resolver.hosters(account_id).await
    }

    /// Probes links through the account's provider resolver (no download, no domain filter:
    /// multihosters check foreign hosters).
    pub async fn check(
        &self,
        account_id: AccountId,
        urls: Vec<Url>,
    ) -> Result<Vec<LinkCheckResult>, Failure> {
        let (urls, mut unknown) = crate::foreign_address::checkable(urls);
        if urls.is_empty() {
            return Ok(unknown);
        }
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self.provider_resolver(&provider)?;
        let mut checked = resolver
            .check(CheckRequest {
                urls,
                client: ClientIdentity {
                    account_id: Some(account_id),
                    proxy_profile_id: None,
                    tls_revision: 0,
                },
            })
            .await?;
        checked.append(&mut unknown);
        Ok(checked)
    }

    /// Returns the manifest concurrency route selected by an account.
    pub async fn concurrency_route(
        &self,
        account_id: Option<AccountId>,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<(ResolverPin, u32)>, Failure> {
        let Some(account_id) = account_id else {
            return Ok(None);
        };
        let provider = account_provider(&self.database, account_id).await?;
        Ok(self.resolver_for_provider(&provider, pin)?.map(|resolver| {
            let metadata = resolver.metadata();
            (
                ResolverPin {
                    plugin_id: metadata.plugin_id,
                    version: metadata.version.clone(),
                },
                metadata.max_concurrent_downloads,
            )
        }))
    }

    /// The unpinned resolver of an account's provider, or why there is none.
    fn provider_resolver(&self, provider: &str) -> Result<Arc<dyn Resolver>, Failure> {
        self.resolver_for_provider(provider, None)?
            .ok_or_else(|| self.missing_resolver(provider))
    }

    /// No resolver serves `provider`. When its plugin is installed and only waits for the next
    /// start (RD-170-12), that is what the person reads — "no resolver is installed" is untrue
    /// then, and it sent the owner looking for a plugin that was right there.
    fn missing_resolver(&self, provider: &str) -> Failure {
        let waiting = rd_provider_registry::by_slug(provider.trim())
            .and_then(|spec| spec.plugin_id)
            .and_then(|id| id.parse::<rd_core::PluginId>().ok())
            .is_some_and(|id| {
                self.waiting
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(&id)
            });
        if waiting {
            return Failure::coded(
                FailureKind::Unsupported,
                "plugin.installed_not_running",
                "The plugin for this provider is installed; it runs after the next restart",
            );
        }
        Failure::coded(
            FailureKind::Unsupported,
            "plugin.resolver_missing",
            "No resolver is installed for this provider",
        )
    }

    fn resolver_for_provider(
        &self,
        provider: &str,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<Arc<dyn Resolver>>, Failure> {
        let chain = self.chain();
        let resolver = chain
            .resolvers
            .iter()
            .find(|resolver| {
                let provider_matches = resolver
                    .metadata()
                    .provider_slug
                    .eq_ignore_ascii_case(provider.trim());
                provider_matches && chain.admits(resolver, pin)
            })
            .cloned();
        if pin.is_some() && resolver.is_none() {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.pinned_version_missing",
                "The plugin version pinned to the job is not installed",
            ));
        }
        Ok(resolver)
    }
}

/// Classifies an unresolvable link: hoster links need a plugin to become transferable and
/// must fail loudly, while a plain HTTP URL is downloaded as it is.
///
/// The provider registry is the only authority consulted here. Asking the resolvers whether
/// one `matches()` the URL looks equivalent but is not: a multihoster accepts any host by
/// declaring a `*` domain, so every plain link would be reported as needing an account and
/// direct downloads would stop working entirely. `provider_for_url` claims a URL only for a
/// `Hoster`-kind provider that lists its host explicitly, which is exactly the question —
/// and installed plugins are covered too, since their manifests contribute registry rows.
fn account_required(url: &Url) -> Option<Failure> {
    let spec = rd_provider_registry::provider_for_url(url)?;
    let host = url.host_str().unwrap_or_default().to_owned();
    Some(
        Failure::coded(
            FailureKind::AuthRequired,
            "resolve.account_required",
            "This hoster needs an account or a free download plugin",
        )
        .with_param("host", host)
        .with_param("provider", spec.slug),
    )
}

/// Puts the bundled provider rows into the registry, once per test process.
///
/// Until RD-101-13 eleven of them were compiled into `rd-provider-registry`, so a unit test
/// asking whether `ddownload.com` may be requested simply got an answer. A provider now exists
/// only while its plugin is installed, so a test that needs one has to say so — which is also
/// the state these tests are meant to reproduce.
#[cfg(test)]
pub(crate) fn register_bundled_providers_for_tests() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let rows: Vec<_> = std::fs::read_dir(root)
            .expect("plugins directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("manifest.toml"))
            .filter(|path| path.is_file())
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .filter_map(|text| toml::from_str::<crate::PluginManifest>(&text).ok())
            // The registered-application OAuth shape no bundled plugin has since 1.5.0; its host
            // mechanics are tested against this fixture.
            .chain(std::iter::once(
                toml::from_str::<crate::PluginManifest>(include_str!(
                    "fixtures/oauth_registered_app.toml"
                ))
                .expect("the registered-application fixture"),
            ))
            .collect::<Vec<_>>();
        rd_provider_registry::replace_secret_fragment_hosts(
            rows.iter()
                .flat_map(|manifest| manifest.secret_fragment_domains.iter().cloned())
                .collect(),
        );
        let rows: Vec<_> = rows
            .iter()
            .filter_map(crate::provider_spec_from_manifest)
            .collect();
        assert!(!rows.is_empty(), "expected bundled provider rows");
        let rejected = rd_provider_registry::replace_dynamic(rows);
        assert!(rejected.is_empty(), "rejected rows: {rejected:?}");
    });
}

fn compatible_components(
    registry: &crate::PluginTypeRegistry,
    host: Arc<dyn ResolverHost>,
    log: Arc<crate::ExecutionLog>,
) -> Vec<(Arc<dyn Resolver>, crate::VersionRole)> {
    registry.instantiate_with_roles(&crate::PluginType::Resolver, |package| {
        let resolver = crate::ComponentResolver::new(
            package.manifest.clone(),
            &package.component,
            Arc::clone(&host),
        )?
        .with_execution_log(Arc::clone(&log));
        Ok(Arc::new(resolver) as Arc<dyn Resolver>)
    })
}

/// Enabled account's provider slug, shared by resolver dispatch and the host's secret gating.
pub(super) async fn account_provider(
    database: &rd_db::Database,
    id: AccountId,
) -> Result<String, Failure> {
    account_credentials(database, id)
        .await
        .map(|(provider, _)| provider)
}

/// Enabled account's provider slug together with the credential mode it stores.
///
/// The mode decides which of a two-mode provider's secret slots is live, so every gate that
/// asks "may this credential go here" needs it alongside the slug.
pub(super) async fn account_credentials(
    database: &rd_db::Database,
    id: AccountId,
) -> Result<(String, Option<rd_provider_registry::CredentialMode>), Failure> {
    database
        .list_accounts()
        .await
        .map_err(permanent)?
        .into_iter()
        .find(|account| account.id == id && account.enabled)
        .map(|account| (account.provider, account.credential_mode))
        .ok_or_else(|| {
            Failure::coded(
                FailureKind::AccountInvalid,
                "plugin.account_unavailable",
                "Account is not available",
            )
        })
}

/// Enabled account's stored username, for `{{username}}` template expansion.
pub(super) async fn account_username(
    database: &rd_db::Database,
    id: AccountId,
) -> Result<Option<String>, Failure> {
    database
        .list_accounts()
        .await
        .map_err(permanent)?
        .into_iter()
        .find(|account| account.id == id && account.enabled)
        .map(|account| account.username)
        .ok_or_else(|| {
            Failure::coded(
                FailureKind::AccountInvalid,
                "plugin.account_unavailable",
                "Account is not available",
            )
        })
}

pub(super) fn permanent(error: impl std::fmt::Display) -> Failure {
    Failure::new(FailureKind::Permanent, error.to_string())
}

#[cfg(test)]
mod tests {
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
}

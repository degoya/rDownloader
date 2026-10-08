use std::{
    collections::HashSet,
    sync::{Arc, PoisonError, RwLock},
};

use rd_core::{AccountId, Failure, FailureKind, ResolverPin};
use rd_http::{ClientPool, SharedNetworkDefaults};
use rd_plugin_api::{Resolver, ResolverHost};
use url::Url;

#[cfg(test)]
mod bundled_headers_tests;
mod dispatch;
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
pub(crate) use host::{with_expanded_credentials, with_own_network, with_response_allowance};
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
mod tests;

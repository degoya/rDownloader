//! Queue runner for downloads carried by an installed transfer backend.
//!
//! The division of labour is deliberate and is the whole point of the type: the plugin moves
//! bytes, the host decides where they land and whether they count. Probing, resume validation,
//! length verification and the atomic promotion into the package folder all happen here, in
//! code the plugin cannot reach — so a backend that lies about being finished produces a
//! failed job, not a truncated file presented as complete.

mod runner;

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_host::{PluginInstaller, PluginType, TransferBackend};

pub use runner::PluginTransferRunner;

/// The installed transfer backends, addressed the two ways the scheduler needs them.
#[derive(Clone)]
pub struct TransferBackends {
    backends: Arc<Vec<Arc<TransferBackend>>>,
    /// Whether loopback and private-network targets may be dialled, from the service's
    /// `--plugin-allow-local-targets`. Deliberately not tied to development mode: accepting
    /// an unsigned package says nothing about where the signed ones may connect.
    allow_local_targets: bool,
}

impl TransferBackends {
    /// Loads and compiles every installed package that declares itself a transfer backend.
    ///
    /// A package that fails to compile is skipped with a warning, exactly like a resolver
    /// component: one broken backend must not cost the queue every other protocol.
    pub async fn load(installer: &PluginInstaller, allow_local_targets: bool) -> Result<Self> {
        let registry = rd_plugin_host::PluginTypeRegistry::load(installer).await?;
        Ok(Self::from_registry(&registry, allow_local_targets))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &rd_plugin_host::PluginTypeRegistry,
        allow_local_targets: bool,
    ) -> Self {
        let backends = registry.instantiate(&PluginType::Transfer, |package| {
            let backend = TransferBackend::new(package.manifest.clone(), &package.component)?;
            Ok(Arc::new(backend))
        });
        Self {
            backends: Arc::new(backends),
            allow_local_targets,
        }
    }

    /// Builds a set from already-compiled backends, for the contract tests.
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(backends: Vec<Arc<TransferBackend>>, allow_local_targets: bool) -> Self {
        Self {
            backends: Arc::new(backends),
            allow_local_targets,
        }
    }

    /// Whether any backend is installed at all, so the runner can stay unregistered otherwise.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    /// URL schemes the installed backends claim, for link intake.
    #[must_use]
    pub fn schemes(&self) -> Vec<String> {
        let mut schemes: Vec<String> = self
            .backends
            .iter()
            .filter_map(|backend| backend.manifest().transfer.as_ref())
            .flat_map(|transfer| transfer.schemes.iter().cloned())
            .collect();
        schemes.sort();
        schemes.dedup();
        schemes
    }

    /// The newest installed backend claiming `scheme`.
    ///
    /// `load_verified` yields the newest version of each plugin first, so the first match is
    /// the one a fresh job should start on.
    #[must_use]
    pub fn for_scheme(&self, scheme: &str) -> Option<&Arc<TransferBackend>> {
        self.backends.iter().find(|backend| backend.claims(scheme))
    }

    /// The exact version a running job is pinned to, or `None` once it is gone.
    #[must_use]
    pub fn pinned(&self, plugin_id: &str, version: &str) -> Option<&Arc<TransferBackend>> {
        self.backends.iter().find(|backend| {
            backend.manifest().id.to_string() == plugin_id && backend.manifest().version == version
        })
    }

    #[must_use]
    pub(crate) fn allows_local_targets(&self) -> bool {
        self.allow_local_targets
    }

    /// The strictest concurrency any installed backend declares.
    ///
    /// One semaphore covers the whole download kind, so a backend that says "one at a time"
    /// has to hold for all of them; the alternative is a per-plugin semaphore, which the
    /// scheduler's registry does not have and this type does not yet need.
    #[must_use]
    pub(crate) fn concurrency_floor(&self) -> Option<usize> {
        self.backends
            .iter()
            .map(|backend| backend.manifest().max_concurrent_downloads as usize)
            .min()
    }
}

/// Builds the runner registered with the scheduler.
#[must_use]
pub fn build(
    backends: TransferBackends,
    database: rd_db::Database,
    custom_ca_pem: Vec<Vec<u8>>,
) -> Arc<dyn rd_scheduler::ExternalRunner> {
    Arc::new(PluginTransferRunner::new(backends, database, custom_ca_pem))
}

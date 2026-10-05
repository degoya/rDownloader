//! Wasmtime Component Model runtime with deny-by-default capabilities.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use rd_plugin_api::{ClientIdentity, ResolverHost};
use wasmtime::{
    Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline, component::Component,
};

use crate::{PluginLimits, engine::SharedEngine};

mod imports;

use imports::{allowed_imports, import_matches};

pub(crate) const EPOCH_TICK: Duration = Duration::from_millis(10);
/// Longest plugin log line the host keeps, in characters.
const MAX_LOG_CHARS: usize = 4096;

/// Per-invocation state exposed only to explicitly linked host functions.
pub struct PluginStoreState {
    limits: StoreLimits,
    allowed_domains: Arc<[String]>,
    max_response_bytes: u64,
    response_bytes: u64,
    host: Option<Arc<dyn ResolverHost>>,
    identity: ClientIdentity,
    redactions: Vec<String>,
    /// Wall-clock instant at which the guest's execution budget runs out. Hoster countdowns
    /// push it forward so a legitimate wait cannot be mistaken for a hung plugin.
    execution_deadline: Instant,
    /// Waiting time still available to this invocation.
    wait_budget: Duration,
    /// Present only while a transfer backend is running; a resolver store has none, which is
    /// why a resolver cannot reach the sink even if it somehow linked the interface.
    pub(crate) transfer: Option<crate::transfer::TransferState>,
    /// Present only while a post-processing or storage plugin is running. A store without
    /// one cannot read a package even if the interface were somehow linked.
    pub(crate) source: Option<crate::extension::SourceState>,
    /// Which allowlist decides where this invocation's requests may go.
    pub(crate) request_authority: rd_plugin_api::RequestAuthority,
    /// Whether this invocation may use the methods that write at the far end.
    pub(crate) write_methods: bool,
    /// Whether this invocation's requests may reach the person's own network (RA-HOST-01).
    ///
    /// Only where the manifest's `*` was narrowed to an address the person supplied — their
    /// WebDAV server, the Nextcloud they crawl, their ntfy — since that server is on the LAN as
    /// often as not. A domain a plugin named itself reaches public addresses only, and this
    /// machine is never reachable, whichever the case (`native::host::address_policy`).
    pub(crate) own_network: bool,
    /// One vault reference this invocation may expand, chosen by the host.
    ///
    /// A resolver's secret is found through its account and its provider; a notification
    /// destination or a storage target has no account, so what it may reach has to be said
    /// per invocation instead. Exactly one, so "which secret" is never a question the plugin
    /// gets to answer.
    pub(crate) granted_secret: Option<String>,
    /// The name the source of the remote job being submitted was added under, answered by
    /// `job-context.source-name`. Set only for a `submit` call; `None` everywhere else.
    pub(crate) job_source_name: Option<String>,
    /// The settings of the notification target being delivered to, already resolved against
    /// the manifest's defaults, answered by `destination-settings.setting` (RD-170-09). Empty
    /// for every other call.
    pub(crate) destination_settings: Vec<(String, String)>,
    /// Sockets the host opened for this invocation, addressed by the opaque handles the
    /// guest received. Dropping the store closes every one of them.
    pub(crate) connections: std::collections::HashMap<u32, crate::transfer::HostConnection>,
    /// Next handle to hand out. Never reused within an invocation, so a stale handle is an
    /// error rather than a different socket.
    pub(crate) next_connection: u32,
}

impl PluginStoreState {
    /// Grants the plugin a hoster countdown: the waited time is taken off the wait budget
    /// and added to the execution deadline. `None` means the budget is exhausted.
    pub(crate) fn claim_wait(&mut self, requested: Duration) -> Option<Duration> {
        if requested > self.wait_budget {
            return None;
        }
        self.wait_budget -= requested;
        self.execution_deadline += requested;
        Some(requested)
    }

    /// Remaining waiting time, for diagnostics and tests.
    #[must_use]
    pub fn wait_budget(&self) -> Duration {
        self.wait_budget
    }

    /// Credits time spent inside a host call back to the execution deadline.
    ///
    /// The epoch counter runs while the guest is parked in a socket read, so without this a
    /// slow server would trip the *compute* timeout. The guest's own instruction budget is
    /// untouched — fuel still runs out on a spinning plugin — but waiting on the network no
    /// longer counts as thinking.
    pub(crate) fn credit_host_time(&mut self, elapsed: Duration) {
        self.execution_deadline += elapsed;
    }
}

impl PluginStoreState {
    /// Domains declared by the pinned plugin manifest.
    #[must_use]
    pub fn allowed_domains(&self) -> &[String] {
        &self.allowed_domains
    }

    /// Accounts bytes returned through controlled HTTP host calls.
    pub fn account_response_bytes(&mut self, bytes: usize) -> Result<()> {
        let bytes = u64::try_from(bytes).context("plugin response length exceeds u64")?;
        self.response_bytes = self
            .response_bytes
            .checked_add(bytes)
            .context("plugin response byte counter overflow")?;
        if self.response_bytes > self.max_response_bytes {
            bail!("plugin HTTP response limit exceeded");
        }
        Ok(())
    }

    /// Response bytes the manifest still allows this invocation (PLUG-06).
    pub(crate) fn remaining_response_bytes(&self) -> u64 {
        self.max_response_bytes.saturating_sub(self.response_bytes)
    }

    /// Number of response bytes delivered during this invocation.
    #[must_use]
    pub fn response_bytes(&self) -> u64 {
        self.response_bytes
    }

    pub(crate) fn host(&self) -> Option<Arc<dyn ResolverHost>> {
        self.host.clone()
    }

    pub(crate) fn identity(&self) -> &ClientIdentity {
        &self.identity
    }

    /// The one vault reference this invocation may expand into `{{secret}}`.
    pub(crate) fn granted_secret(&self) -> Option<&str> {
        self.granted_secret.as_deref()
    }

    pub(crate) fn request_authority(&self) -> rd_plugin_api::RequestAuthority {
        self.request_authority
    }

    pub(crate) fn write_methods(&self) -> bool {
        self.write_methods
    }

    pub(crate) fn own_network(&self) -> bool {
        self.own_network
    }

    pub(crate) fn remember_redactions(&mut self, values: impl IntoIterator<Item = String>) {
        self.redactions
            .extend(values.into_iter().filter(|value| value.len() >= 4));
    }

    /// Masks every remembered secret in a plugin's log line, then cuts it to
    /// [`MAX_LOG_CHARS`].
    ///
    /// The cut is made in the line's *original* positions (RA-HOST-03): the secrets are found
    /// in the line as the plugin wrote it, a secret that starts before the cut is masked whole,
    /// and nothing that started past the cut is kept. Masking first and cutting the masked text
    /// (PLUG-18) let a mask shorter than its secret pull text from beyond the cut back in front
    /// of it — a third secret there, half outside the scanned part, was logged as its prefix.
    /// The input is still bounded, to the cut plus the longest secret, which is as far as a
    /// secret starting before the cut can reach, so a plugin cannot make the host scan a line
    /// of any length.
    pub(crate) fn redact_log(&self, message: &str) -> String {
        let longest = self
            .redactions
            .iter()
            .map(|secret| secret.chars().count())
            .max()
            .unwrap_or(0);
        let byte_at = |chars: usize| {
            message
                .char_indices()
                .nth(chars)
                .map_or(message.len(), |(index, _)| index)
        };
        let cut = byte_at(MAX_LOG_CHARS);
        let scanned = &message[..byte_at(MAX_LOG_CHARS.saturating_add(longest))];
        let mut masked = self
            .redactions
            .iter()
            .flat_map(|secret| {
                scanned
                    .match_indices(secret.as_str())
                    .map(|(start, found)| (start, start + found.len()))
            })
            .collect::<Vec<_>>();
        masked.sort_unstable();
        let mut logged = String::new();
        let mut kept_to = 0;
        for (start, end) in masked {
            if start >= cut {
                break;
            }
            if end <= kept_to {
                continue;
            }
            if start >= kept_to {
                logged.push_str(&scanned[kept_to..start]);
                logged.push_str("[REDACTED]");
            }
            kept_to = end;
        }
        if kept_to < cut {
            logged.push_str(&scanned[kept_to..cut]);
        }
        // Only ever shortens text that is already masked, so it cannot bring a secret back.
        logged.chars().take(MAX_LOG_CHARS).collect()
    }
}

/// Compiles Components and creates resource-limited stores.
///
/// Cheap to create: the engine behind it is the process's one shared engine (`crate::engine`),
/// and what is per plugin here is only the limits its stores are given.
pub struct SandboxEngine {
    shared: Arc<SharedEngine>,
    limits: PluginLimits,
}

impl SandboxEngine {
    /// A sandbox on the shared Component Model engine, with fuel and epoch interruption.
    pub fn new(limits: PluginLimits) -> Result<Self> {
        validate_limits(limits)?;
        Ok(Self {
            shared: crate::engine::shared()?,
            limits,
        })
    }

    /// Compiles one binary Component and rejects every import the manifest did not grant.
    ///
    /// This is the first of the two places a grant is enforced — the linker is the second.
    /// Checking here means a package that reaches beyond its manifest is refused at install
    /// and packaging time, where its author sees it, rather than mid-download.
    pub fn compile_component(
        &self,
        bytes: &[u8],
        manifest: &crate::PluginManifest,
    ) -> Result<Component> {
        // Compiled once per content; the grant is checked on every call, because the same
        // bytes may arrive under a different manifest.
        let component = self.shared.compile(bytes)?;
        let allowed = allowed_imports(manifest);
        for (name, _) in component.component_type().imports(self.engine()) {
            if !allowed.iter().any(|allowed| import_matches(name, allowed)) {
                bail!("component imports forbidden interface {name}");
            }
        }
        Ok(component)
    }

    /// Creates a fresh store without a resolver host, for the sandbox tests.
    #[cfg(test)]
    pub fn create_store(&self, allowed_domains: Vec<String>) -> Result<Store<PluginStoreState>> {
        self.create_store_inner(
            allowed_domains,
            None,
            ClientIdentity {
                account_id: None,
                proxy_profile_id: None,
                tls_revision: 0,
            },
        )
    }

    /// Creates a fresh store connected only to the controlled resolver host capabilities.
    pub fn create_invocation_store(
        &self,
        allowed_domains: Vec<String>,
        host: Arc<dyn ResolverHost>,
        identity: ClientIdentity,
    ) -> Result<Store<PluginStoreState>> {
        self.create_store_inner(allowed_domains, Some(host), identity)
    }

    fn create_store_inner(
        &self,
        allowed_domains: Vec<String>,
        host: Option<Arc<dyn ResolverHost>>,
        identity: ClientIdentity,
    ) -> Result<Store<PluginStoreState>> {
        let memory_bytes = usize::try_from(self.limits.memory_bytes)
            .context("plugin memory limit exceeds platform usize")?;
        let state = PluginStoreState {
            limits: StoreLimitsBuilder::new()
                .memory_size(memory_bytes)
                .instances(32)
                .tables(8)
                .memories(1)
                .trap_on_grow_failure(true)
                .build(),
            allowed_domains: allowed_domains.into(),
            max_response_bytes: self.limits.max_response_bytes,
            response_bytes: 0,
            host,
            identity,
            redactions: Vec::new(),
            granted_secret: None,
            job_source_name: None,
            destination_settings: Vec::new(),
            request_authority: rd_plugin_api::RequestAuthority::Provider,
            write_methods: false,
            own_network: false,
            execution_deadline: Instant::now()
                + Duration::from_millis(self.limits.timeout_milliseconds),
            wait_budget: Duration::from_millis(self.limits.wait_budget_milliseconds),
            transfer: None,
            source: None,
            connections: std::collections::HashMap::new(),
            next_connection: 1,
        };
        let mut store = Store::new(self.engine(), state);
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.limits.fuel)
            .map_err(|error| anyhow::anyhow!("configure plugin fuel: {error}"))?;
        store.set_epoch_deadline(timeout_ticks(self.limits.timeout_milliseconds));
        // The epoch counter keeps ticking while the guest is parked in a host call, so a
        // hoster countdown would trip the plain timeout trap. Consult the wall-clock
        // deadline instead, which waits push forward: compute time stays bounded, waiting
        // does not count against it.
        store.epoch_deadline_callback(|context| {
            let deadline = context.data().execution_deadline;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(UpdateDeadline::Interrupt);
            }
            Ok(UpdateDeadline::Continue(timeout_ticks(
                u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX),
            )))
        });
        Ok(store)
    }

    /// Creates a store for one transfer attempt, carrying its destination and limits.
    pub fn create_transfer_store(
        &self,
        transfer: crate::transfer::TransferState,
    ) -> Result<Store<PluginStoreState>> {
        let mut store = self.create_store_inner(
            Vec::new(),
            None,
            ClientIdentity {
                account_id: None,
                proxy_profile_id: None,
                tls_revision: 0,
            },
        )?;
        store.data_mut().transfer = Some(transfer);
        Ok(store)
    }

    /// A store for an extension invocation that reads one package.
    pub fn create_source_store(
        &self,
        allowed_domains: Vec<String>,
        host: Option<Arc<dyn ResolverHost>>,
        identity: ClientIdentity,
        granted_secret: Option<String>,
        write_methods: bool,
        source: crate::extension::SourceState,
    ) -> Result<Store<PluginStoreState>> {
        let mut store = self.create_store_inner(allowed_domains, host, identity)?;
        store.data_mut().granted_secret = granted_secret;
        store.data_mut().request_authority = rd_plugin_api::RequestAuthority::Manifest;
        store.data_mut().write_methods = write_methods;
        store.data_mut().source = Some(source);
        Ok(store)
    }

    /// A store for an extension invocation that reads no package.
    ///
    /// An extension type reaches the network the same way a resolver does — through the host,
    /// confined to the domains its own manifest declares — so it is given the same two things
    /// rather than an empty list, which would have left every type but intake unable to do
    /// the one thing it exists for.
    pub fn create_extension_store(
        &self,
        allowed_domains: Vec<String>,
        host: Option<Arc<dyn ResolverHost>>,
        identity: ClientIdentity,
        granted_secret: Option<String>,
        write_methods: bool,
    ) -> Result<Store<PluginStoreState>> {
        let mut store = self.create_store_inner(allowed_domains, host, identity)?;
        store.data_mut().granted_secret = granted_secret;
        store.data_mut().request_authority = rd_plugin_api::RequestAuthority::Manifest;
        store.data_mut().write_methods = write_methods;
        Ok(store)
    }

    /// Returns the underlying engine for generated WIT linkers.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        self.shared.engine()
    }
}

fn validate_limits(limits: PluginLimits) -> Result<()> {
    if limits.memory_bytes == 0
        || limits.fuel == 0
        || limits.timeout_milliseconds == 0
        || limits.max_response_bytes == 0
    {
        bail!("plugin limits must be non-zero");
    }
    Ok(())
}

fn timeout_ticks(milliseconds: u64) -> u64 {
    milliseconds
        .saturating_add(EPOCH_TICK.as_millis() as u64 - 1)
        .checked_div(EPOCH_TICK.as_millis() as u64)
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

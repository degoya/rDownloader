//! Wasmtime Component Model runtime with deny-by-default capabilities.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use rd_plugin_api::{ClientIdentity, ResolverHost};
use wasmtime::{
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline, component::Component,
};

use crate::PluginLimits;

const EPOCH_TICK: Duration = Duration::from_millis(10);
/// Interfaces every plugin may import, whatever its manifest says. Neither reaches the
/// network, a credential value or the user.
const BASE_IMPORTS: [&str; 2] = ["rdownloader:plugin/host", "rdownloader:plugin/types"];

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
    /// One vault reference this invocation may expand, chosen by the host.
    ///
    /// A resolver's secret is found through its account and its provider; a notification
    /// destination or a storage target has no account, so what it may reach has to be said
    /// per invocation instead. Exactly one, so "which secret" is never a question the plugin
    /// gets to answer.
    pub(crate) granted_secret: Option<String>,
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

    pub(crate) fn remember_redactions(&mut self, values: impl IntoIterator<Item = String>) {
        self.redactions
            .extend(values.into_iter().filter(|value| value.len() >= 4));
    }

    pub(crate) fn redact_log(&self, message: &str) -> String {
        let mut redacted = message.chars().take(4096).collect::<String>();
        for secret in &self.redactions {
            redacted = redacted.replace(secret, "[REDACTED]");
        }
        redacted
    }
}

/// Compiles Components and creates resource-limited stores.
pub struct SandboxEngine {
    engine: Engine,
    limits: PluginLimits,
    stop_ticker: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

impl SandboxEngine {
    /// Creates a Component Model engine with fuel and epoch interruption enabled.
    pub fn new(limits: PluginLimits) -> Result<Self> {
        validate_limits(limits)?;
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .cranelift_nan_canonicalization(true);
        let engine = Engine::new(&config)
            .map_err(|error| anyhow::anyhow!("create Wasmtime sandbox engine: {error}"))?;
        let stop_ticker = Arc::new(AtomicBool::new(false));
        let ticker = spawn_epoch_ticker(engine.clone(), Arc::clone(&stop_ticker));
        Ok(Self {
            engine,
            limits,
            stop_ticker,
            ticker: Some(ticker),
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
        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|error| anyhow::anyhow!("compile WebAssembly component: {error}"))?;
        let allowed = allowed_imports(manifest);
        for (name, _) in component.component_type().imports(&self.engine) {
            if !allowed.iter().any(|allowed| import_matches(name, allowed)) {
                bail!("component imports forbidden interface {name}");
            }
        }
        Ok(component)
    }

    /// Creates a fresh store for one resolver call.
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
            request_authority: rd_plugin_api::RequestAuthority::Provider,
            write_methods: false,
            execution_deadline: Instant::now()
                + Duration::from_millis(self.limits.timeout_milliseconds),
            wait_budget: Duration::from_millis(self.limits.wait_budget_milliseconds),
            transfer: None,
            source: None,
            connections: std::collections::HashMap::new(),
            next_connection: 1,
        };
        let mut store = Store::new(&self.engine, state);
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
        &self.engine
    }
}

impl Drop for SandboxEngine {
    fn drop(&mut self) {
        self.stop_ticker.store(true, Ordering::Release);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
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

/// The interfaces one manifest permits: the always-available base, the interface its plugin
/// type owns, and one entry per declared capability. WASI is in none of them, so it stays
/// denied by construction.
fn allowed_imports(manifest: &crate::PluginManifest) -> Vec<&'static str> {
    let mut allowed = BASE_IMPORTS.to_vec();
    // The destination of a transfer is not a capability a plugin asks for; it is what a
    // transfer backend *is*. A resolver importing it would be reaching for a file to write.
    if manifest.plugin_type == crate::PluginType::Transfer {
        allowed.push("rdownloader:plugin/sink");
    }
    // Reading the files of a package is likewise not a grant but a definition: it is what a
    // post-processing step and a storage destination do. Neither can name a file — they are
    // handed a package-scoped handle — so the import is safe to give unconditionally to
    // exactly these two types and to nobody else.
    if matches!(
        manifest.plugin_type,
        crate::PluginType::Postprocess | crate::PluginType::Storage
    ) {
        allowed.push("rdownloader:plugin/source");
    }
    // Writing back what a flow produced is what an authentication plugin is; no other type
    // may even name the interface, and the plugin still names no reference of its own.
    if matches!(
        manifest.plugin_type,
        crate::PluginType::Auth | crate::PluginType::OAuth
    ) {
        allowed.push("rdownloader:plugin/credentials");
    }
    let capabilities = &manifest.capabilities;
    if capabilities.net_http.is_some() {
        allowed.push("rdownloader:plugin/http");
    }
    if capabilities.cookies {
        allowed.push("rdownloader:plugin/cookies");
    }
    if capabilities.captcha {
        allowed.push("rdownloader:plugin/captcha");
    }
    if capabilities.net_stream.is_some() {
        allowed.push("rdownloader:plugin/net");
    }
    // Computing over a credential the guest never sees (RD-120-20). A grant like any other:
    // a component that imports the interface without declaring it is refused here, at
    // install and packaging time, rather than mid sign-in.
    if capabilities.key_derivation {
        allowed.push("rdownloader:plugin/key-derivation");
    }
    allowed
}

fn import_matches(name: &str, allowed: &str) -> bool {
    name == allowed
        || name
            .strip_prefix(allowed)
            .is_some_and(|rest| rest.starts_with('@'))
}

fn timeout_ticks(milliseconds: u64) -> u64 {
    milliseconds
        .saturating_add(EPOCH_TICK.as_millis() as u64 - 1)
        .checked_div(EPOCH_TICK.as_millis() as u64)
        .unwrap_or(1)
        .max(1)
}

fn spawn_epoch_ticker(engine: Engine, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(EPOCH_TICK);
            engine.increment_epoch();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{PluginLimits, SandboxEngine, allowed_imports, timeout_ticks};

    /// Writing a credential back is bound to the authentication type, not to a capability.
    /// A manifest cannot ask for it, so the only way to reach it is to be that type.
    #[test]
    fn only_an_authentication_plugin_may_write_a_credential() {
        const CREDENTIALS: &str = "rdownloader:plugin/credentials";
        let manifest = |plugin_type: &str| -> crate::PluginManifest {
            toml::from_str(&format!(
                r#"
                manifest_version = 3
                plugin_type = "{plugin_type}"
                api_version = "0.9.0"
                id = "11111111-1111-4111-8111-111111111111"
                name = "Demo"
                version = "0.1.0"
                key_id = "demo-v1"
                public_key = "AAAA"
                [metadata]
                description = "d"
                author = "a"
                license = "MIT"
                min_app_version = "0.8.0"
                "#
            ))
            .expect("manifest parses")
        };
        assert!(
            allowed_imports(&manifest("auth")).contains(&CREDENTIALS),
            "an authentication plugin needs it"
        );
        for other in [
            "resolver",
            "transfer",
            "intake",
            "enricher",
            "notifier",
            "postprocess",
            "storage",
        ] {
            assert!(
                !allowed_imports(&manifest(other)).contains(&CREDENTIALS),
                "{other} must not reach the credential store"
            );
        }
    }

    #[test]
    fn timeout_is_rounded_up_to_epoch_ticks() {
        assert_eq!(timeout_ticks(1), 1);
        assert_eq!(timeout_ticks(10), 1);
        assert_eq!(timeout_ticks(11), 2);
    }

    #[test]
    fn response_budget_is_cumulative() {
        let limits = PluginLimits {
            max_response_bytes: 8,
            ..PluginLimits::default()
        };
        let sandbox = SandboxEngine::new(limits).expect("sandbox");
        let mut store = sandbox
            .create_store(vec!["example.test".to_owned()])
            .expect("store");
        store
            .data_mut()
            .account_response_bytes(5)
            .expect("first response");
        assert!(store.data_mut().account_response_bytes(4).is_err());
    }

    /// Time spent inside a host call is waiting, not computing. A resolver's HTTP request was
    /// never credited, so a slow hoster burned the same 15s budget as an endless loop — with
    /// ten downloads at once several tripped `plugin.timeout` while a retry worked fine.
    /// Unlike a hoster countdown this must not touch the wait budget, which exists for
    /// guest-requested waits.
    #[test]
    fn a_host_call_extends_the_deadline_without_spending_the_wait_budget() {
        let limits = PluginLimits {
            timeout_milliseconds: 15_000,
            wait_budget_milliseconds: 60_000,
            ..PluginLimits::default()
        };
        let sandbox = SandboxEngine::new(limits).expect("sandbox");
        let mut store = sandbox.create_store(Vec::new()).expect("store");
        let deadline_before = store.data().execution_deadline;

        store
            .data_mut()
            .credit_host_time(std::time::Duration::from_secs(12));

        assert_eq!(
            store.data().execution_deadline - deadline_before,
            std::time::Duration::from_secs(12),
            "a 12s HTTP wait must buy 12s of extra execution time"
        );
        assert_eq!(
            store.data().wait_budget(),
            std::time::Duration::from_secs(60),
            "host time is not a guest-requested wait and must leave the budget untouched"
        );
    }

    /// A hoster countdown must come out of the wait budget and buy the same amount of extra
    /// execution time, so waiting is never mistaken for a hung plugin — and must run out.
    #[test]
    fn waiting_spends_the_budget_and_extends_the_execution_deadline() {
        let limits = PluginLimits {
            timeout_milliseconds: 15_000,
            wait_budget_milliseconds: 60_000,
            ..PluginLimits::default()
        };
        let sandbox = SandboxEngine::new(limits).expect("sandbox");
        let mut store = sandbox.create_store(Vec::new()).expect("store");
        let deadline_before = store.data().execution_deadline;

        let granted = store
            .data_mut()
            .claim_wait(std::time::Duration::from_secs(45))
            .expect("45s fits the budget");

        assert_eq!(granted, std::time::Duration::from_secs(45));
        assert_eq!(
            store.data().execution_deadline - deadline_before,
            std::time::Duration::from_secs(45)
        );
        assert_eq!(
            store.data().wait_budget(),
            std::time::Duration::from_secs(15)
        );
        assert!(
            store
                .data_mut()
                .claim_wait(std::time::Duration::from_secs(16))
                .is_none(),
            "a wait beyond the remaining budget must be refused"
        );
    }

    #[test]
    fn component_log_values_are_redacted_after_cookie_access() {
        let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
        let mut store = sandbox.create_store(Vec::new()).expect("store");
        store
            .data_mut()
            .remember_redactions(["session-secret".to_owned()]);

        assert_eq!(
            store.data().redact_log("cookie=session-secret"),
            "cookie=[REDACTED]"
        );
    }
}

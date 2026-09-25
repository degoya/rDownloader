//! Host side of the transfer-backend contract.
//!
//! A transfer plugin gets two things a resolver does not: raw sockets, and somewhere to put
//! the bytes. Both are host-owned. `net` hands out connection handles the host opened and
//! closes them when the guest drops them; `sink` writes into the part file of the transfer
//! this invocation is running, which the guest can neither name nor reach past. Promotion to
//! the final file never happens in here at all — the runner does it, after the host has
//! checked the length and any checksum.

mod connection;
mod sink;
mod state;

use anyhow::Result;
use wasmtime::component::{HasSelf, Linker};

pub use connection::HostConnection;
pub use exports::rdownloader::plugin::transfer::{Job as TransferJob, RemoteFile};
pub use state::{TransferOutcome, TransferState, TransferTarget};

use crate::{PluginManifest, SandboxEngine, runtime::PluginStoreState};

wasmtime::component::bindgen!({
    path: "../rd-plugin-api/wit",
    world: "transfer-plugin",
    imports: { default: async },
    exports: { default: async },
    with: {
        "rdownloader:plugin/types@0.9.0": crate::component::rdownloader::plugin::types,
        "rdownloader:plugin/host@0.9.0": crate::component::rdownloader::plugin::host,
        "rdownloader:plugin/http@0.9.0": crate::component::rdownloader::plugin::http,
    },
});

/// Largest single read a backend may ask for, so one call cannot claim the whole budget.
pub(crate) const MAX_READ_BYTES: u32 = 1024 * 1024;

/// A compiled transfer backend, pinned to one installed manifest version.
pub struct TransferBackend {
    manifest: PluginManifest,
    sandbox: SandboxEngine,
    pre: TransferPluginPre<PluginStoreState>,
    log: std::sync::Arc<crate::ExecutionLog>,
}

impl TransferBackend {
    /// Compiles a verified package and links only the interfaces its manifest grants.
    pub fn new(manifest: PluginManifest, component_bytes: &[u8]) -> Result<Self> {
        let sandbox = SandboxEngine::new(manifest.limits)?;
        let component = sandbox.compile_component(component_bytes, &manifest)?;
        let mut linker = Linker::new(sandbox.engine());
        rdownloader::plugin::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        rdownloader::plugin::sink::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        if manifest.capabilities.net_stream.is_some() {
            rdownloader::plugin::net::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        }
        if manifest.capabilities.net_http.is_some() {
            rdownloader::plugin::http::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        }
        let pre = TransferPluginPre::new(linker.instantiate_pre(&component)?)?;
        Ok(Self {
            manifest,
            sandbox,
            pre,
            log: crate::ExecutionLog::disabled(),
        })
    }

    /// Records this backend's invocations in the plugin execution history.
    #[must_use]
    pub fn with_execution_log(mut self, log: std::sync::Arc<crate::ExecutionLog>) -> Self {
        self.log = log;
        self
    }

    /// Times one call and files what became of it.
    async fn recorded<T, F>(&self, operation: &'static str, call: F) -> Result<T, rd_core::Failure>
    where
        F: std::future::Future<Output = Result<T, rd_core::Failure>>,
    {
        let invocation = self.log.begin(
            &self.manifest.id.to_string(),
            &self.manifest.name,
            &self.manifest.version,
            self.manifest.plugin_type.as_str(),
            operation,
        );
        let result = call.await;
        self.log.finish(invocation, &result);
        result
    }

    /// The manifest this instance is pinned to.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Whether this backend claims `scheme`.
    #[must_use]
    pub fn claims(&self, scheme: &str) -> bool {
        self.manifest.transfer.as_ref().is_some_and(|transfer| {
            transfer
                .schemes
                .iter()
                .any(|claimed| claimed.eq_ignore_ascii_case(scheme))
        })
    }

    /// Asks the backend what is at `url` without writing anything.
    pub async fn probe(
        &self,
        state: TransferState,
        url: String,
        credential_ref: Option<String>,
    ) -> Result<exports::rdownloader::plugin::transfer::RemoteFile, rd_core::Failure> {
        if crate::foreign_address::carries_marker(&url) {
            return Err(crate::foreign_address::refused());
        }
        self.recorded("probe", self.probe_inner(state, url, credential_ref))
            .await
    }

    /// Runs one transfer attempt and reports how it ended.
    pub async fn run(
        &self,
        state: TransferState,
        job: exports::rdownloader::plugin::transfer::Job,
    ) -> Result<TransferOutcome, rd_core::Failure> {
        if crate::foreign_address::carries_marker(&job.url) {
            return Err(crate::foreign_address::refused());
        }
        self.recorded("run", self.run_inner(state, job)).await
    }

    async fn probe_inner(
        &self,
        state: TransferState,
        url: String,
        credential_ref: Option<String>,
    ) -> Result<exports::rdownloader::plugin::transfer::RemoteFile, rd_core::Failure> {
        let mut store = self.store(state)?;
        let bindings = self.instantiate(&mut store).await?;
        bindings
            .rdownloader_plugin_transfer()
            .call_probe(&mut store, &url, credential_ref.as_deref())
            .await
            .map_err(crate::component::component_failure)?
            .map_err(crate::component::from_wit_failure)
    }

    async fn run_inner(
        &self,
        state: TransferState,
        job: exports::rdownloader::plugin::transfer::Job,
    ) -> Result<TransferOutcome, rd_core::Failure> {
        let mut store = self.store(state)?;
        let bindings = self.instantiate(&mut store).await?;
        let end = bindings
            .rdownloader_plugin_transfer()
            .call_run(&mut store, &job)
            .await
            .map_err(crate::component::component_failure)?;
        state::outcome(store.data().transfer.as_ref().expect("transfer store"), end)
    }

    fn store(
        &self,
        state: TransferState,
    ) -> Result<wasmtime::Store<PluginStoreState>, rd_core::Failure> {
        self.sandbox.create_transfer_store(state).map_err(|error| {
            rd_core::Failure::coded(
                rd_core::FailureKind::Permanent,
                "plugin.execution_failed",
                format!("Plugin execution failed: {error:#}"),
            )
        })
    }

    async fn instantiate(
        &self,
        store: &mut wasmtime::Store<PluginStoreState>,
    ) -> Result<TransferPlugin, rd_core::Failure> {
        self.pre
            .instantiate_async(store)
            .await
            .map_err(crate::component::component_failure)
    }
}

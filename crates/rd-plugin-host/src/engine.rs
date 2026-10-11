//! The one Wasmtime engine every plugin in this process is compiled and run on (RD-130-06).
//!
//! Every `SandboxEngine::new` used to build an engine of its own, epoch-ticker thread and all,
//! and every compile started from nothing: a plugin was compiled once when its package was
//! verified and again by each adapter that ran it. The engine configuration never differed —
//! the limits belong to a store, not to an engine — so one engine serves them all, one ticker
//! drives every deadline, and a component is compiled once per exact byte content and handed
//! out again from memory. What survives a restart is `compile_cache`'s business.
//!
//! Handing a compiled component out again bypasses nothing: it is looked up by the SHA-256 of
//! the very bytes the caller passes, which the caller has verified before asking, exactly as it
//! had to before there was anything to look up.

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, component::Component};

use crate::compile_cache::CompileCache;

/// Distinct component contents kept compiled in memory. An installation holds one per plugin
/// version it runs; past this the map starts over, and an evicted entry costs a disk read.
const MAX_COMPILED: usize = 512;

static SHARED: OnceLock<Arc<SharedEngine>> = OnceLock::new();

/// Keeps compiled plugin code under `directory`, so a restart with unchanged plugins does not
/// compile them again.
///
/// Called once at startup, before anything loads a plugin: the cache is part of the engine's
/// configuration, and the engine is built by the first compile. A later call is refused rather
/// than ignored. A service that never calls this — the CLI, the tests — compiles as before.
pub fn configure_compile_cache(directory: &Path) -> Result<()> {
    let shared = Arc::new(SharedEngine::new(Some(directory))?);
    SHARED.set(shared).map_err(|_| {
        anyhow::anyhow!("the plugin engine was built before its compile cache was configured")
    })
}

/// The engine the service configured with its compile cache, if it did; never builds one.
pub(crate) fn configured() -> Option<Arc<SharedEngine>> {
    SHARED.get().map(Arc::clone)
}

/// The process's engine, built on first use when `configure_compile_cache` was not called.
pub(crate) fn shared() -> Result<Arc<SharedEngine>> {
    if let Some(shared) = SHARED.get() {
        return Ok(Arc::clone(shared));
    }
    // Two first uses at once may both build one; the loser's is dropped with its ticker.
    let _ = SHARED.set(Arc::new(SharedEngine::new(None)?));
    SHARED
        .get()
        .map(Arc::clone)
        .context("the plugin engine is not available")
}

/// An engine, its epoch ticker, and what it has compiled.
pub(crate) struct SharedEngine {
    engine: Engine,
    compiled: Mutex<HashMap<[u8; 32], Component>>,
    disk: Option<CompileCache>,
    /// Compiles that did not come from memory, for the tests that say "once".
    compilations: AtomicUsize,
    stop_ticker: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

impl SharedEngine {
    /// Builds an engine with fuel and epoch interruption, and the disk cache when given one.
    pub(crate) fn new(cache_directory: Option<&Path>) -> Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .cranelift_nan_canonicalization(true);
        // Opened — and swept — before the engine exists, so nothing unchecked is ever read.
        let disk = cache_directory.map(CompileCache::open).transpose()?;
        if let Some(disk) = &disk {
            config.cache(Some(disk.cache().clone()));
        }
        let engine = Engine::new(&config)
            .map_err(|error| anyhow::anyhow!("create Wasmtime sandbox engine: {error}"))?;
        let stop_ticker = Arc::new(AtomicBool::new(false));
        let ticker = spawn_epoch_ticker(engine.clone(), Arc::clone(&stop_ticker));
        Ok(Self {
            engine,
            compiled: Mutex::new(HashMap::new()),
            disk,
            compilations: AtomicUsize::new(0),
            stop_ticker,
            ticker: Some(ticker),
        })
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Compiles `bytes` once per exact content and hands out the same component afterwards.
    ///
    /// The lock is not held across the compile, so two plugins compile side by side; two
    /// callers racing on the *same* bytes both compile and the first result is kept.
    pub(crate) fn compile(&self, bytes: &[u8]) -> Result<Component> {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        if let Some(component) = self.lock()?.get(&digest) {
            return Ok(component.clone());
        }
        // Marked until its entry is recorded, so the entry is known to be this component's
        // (RD-1240-34).
        let _compiling = self
            .disk
            .as_ref()
            .map(|disk| disk.begin(&crate::compile_cache::hex(&digest)));
        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|error| anyhow::anyhow!("compile WebAssembly component: {error}"))?;
        self.compilations.fetch_add(1, Ordering::Relaxed);
        if let Some(disk) = &self.disk {
            disk.record_new_entries();
        }
        let mut compiled = self.lock()?;
        if compiled.len() >= MAX_COMPILED {
            compiled.clear();
        }
        Ok(compiled.entry(digest).or_insert(component).clone())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<[u8; 32], Component>>> {
        self.compiled
            .lock()
            .map_err(|_| anyhow::anyhow!("plugin compile memo lock poisoned"))
    }

    #[cfg(test)]
    pub(crate) fn compilations(&self) -> usize {
        self.compilations.load(Ordering::Relaxed)
    }

    pub(crate) fn disk(&self) -> Option<&CompileCache> {
        self.disk.as_ref()
    }
}

impl Drop for SharedEngine {
    fn drop(&mut self) {
        self.stop_ticker.store(true, Ordering::Release);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
    }
}

fn spawn_epoch_ticker(engine: Engine, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(crate::runtime::EPOCH_TICK);
            engine.increment_epoch();
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::SharedEngine;

    /// The smallest valid component: the preamble of the component encoding and nothing else.
    pub(crate) const EMPTY_COMPONENT: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    /// The same component with a custom section appended, so its bytes — and nothing about
    /// what it does — differ.
    pub(crate) fn variant(tag: u8) -> Vec<u8> {
        let mut bytes = EMPTY_COMPONENT.to_vec();
        // Section id 0 (custom), 3 bytes: a one-byte name "t", then the payload.
        bytes.extend_from_slice(&[0x00, 0x03, 0x01, b't', tag]);
        bytes
    }

    #[test]
    fn a_component_is_compiled_once_per_content() {
        let shared = SharedEngine::new(None).expect("engine");
        shared.compile(&EMPTY_COMPONENT).expect("first compile");
        shared.compile(&EMPTY_COMPONENT).expect("second compile");
        assert_eq!(shared.compilations(), 1, "the same bytes compile once");

        shared.compile(&variant(1)).expect("changed bytes");
        assert_eq!(shared.compilations(), 2, "different bytes compile anew");
    }

    #[test]
    fn a_component_that_does_not_compile_is_not_remembered() {
        let shared = SharedEngine::new(None).expect("engine");
        assert!(shared.compile(b"not a component").is_err());
        assert!(shared.compile(b"not a component").is_err());
        assert_eq!(shared.compilations(), 0);
    }
}

//! Debrid-Link sign-in through the provider's OAuth device flow (RD-090-13).
//!
//! Reading the provider's answers lives in [`flow`], which knows nothing about the plugin
//! contract, so it can be unit-tested without a WebAssembly target. `guest` is the thin
//! component wrapper around it and exists only on `wasm32`.

pub mod flow;

#[cfg(target_arch = "wasm32")]
mod guest;

//! MD5 checksum sidecars as a post-processing step (RD-090-16).
//!
//! The sidecar format lives in [`sidecar`], which knows nothing about the plugin contract, so
//! it can be unit-tested without a WebAssembly target. `guest` is the thin component wrapper
//! around it and exists only on `wasm32`.

pub mod sidecar;

#[cfg(target_arch = "wasm32")]
mod guest;

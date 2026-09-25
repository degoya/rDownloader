//! Tidying release file names as a post-processing step (RD-092-03).
//!
//! The rules live in [`rules`], which knows nothing about the plugin contract, so they can be
//! unit-tested without a WebAssembly target. `guest` is the thin component wrapper around them
//! and exists only on `wasm32`.

pub mod rules;

#[cfg(target_arch = "wasm32")]
mod guest;

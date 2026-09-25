//! Notification destination plugin.
//!
//! The message shaping lives in [`payload`], which knows nothing about the plugin contract,
//! so it can be unit-tested without a WebAssembly target. `guest` is the thin component
//! wrapper around it and exists only on `wasm32`.

pub mod payload;

#[cfg(target_arch = "wasm32")]
mod guest;

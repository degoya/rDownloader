//! Media library refresh for Plex (RD-1240-12), delivered as a notification destination.
//!
//! What the request looks like lives in [`request`], which knows nothing about the plugin
//! contract, so it can be unit-tested without a WebAssembly target. `guest` is the thin component
//! wrapper around it and exists only on `wasm32`.

pub mod request;

#[cfg(target_arch = "wasm32")]
mod guest;

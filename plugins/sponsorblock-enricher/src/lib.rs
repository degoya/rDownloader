//! SponsorBlock metadata enricher (RD-090-14).
//!
//! Reading the video id and the segment list lives in [`segments`], which knows nothing about
//! the plugin contract, so it can be unit-tested without a WebAssembly target. `guest` is the
//! thin component wrapper around it and exists only on `wasm32`.

pub mod segments;

#[cfg(target_arch = "wasm32")]
mod guest;

//! JDownloader `.crawljob` intake parser.
//!
//! The format handling lives in [`parse`], which knows nothing about the plugin contract, so
//! it can be unit-tested without a WebAssembly target. `guest` is the thin component wrapper
//! around it and exists only on `wasm32`.

pub mod parse;

#[cfg(target_arch = "wasm32")]
mod guest;

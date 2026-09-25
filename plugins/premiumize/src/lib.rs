//! Premiumize resolver using the documented Bearer-authenticated API.
//!
//! A multihoster: it claims no host of its own and resolves other hosters' links on the
//! account's behalf, so `matches()` accepts any http(s) URL and the catalogue comes from the
//! account.

mod account;
mod messages;
mod resolver;
mod services;

#[cfg(target_arch = "wasm32")]
mod guest;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::PremiumizeResolver;

//! HitFile resolver: the operator's JSON API, as a guest through the site's own Turnstile and
//! countdown, or with an account.
//!
//! The protocol logic lives in `rd-plugin-turbobit-common`, shared with the operator's other
//! brand; this crate contributes the brand — hosts, id shape, codes, catalogue — and the two
//! adapters, so the native fallback and the WebAssembly component run one piece of code.

mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
mod resolver;

pub use resolver::BRAND;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::HitfileResolver;

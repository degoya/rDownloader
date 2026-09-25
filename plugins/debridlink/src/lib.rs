//! Debrid-Link resolver: Bearer-authenticated JSON API (`debrid-link.com/api/v2`), multihoster,
//! premium API key only. `matches()` accepts any http(s) URL like `premiumize`/`alldebrid`;
//! `resolve()` posts the link to `/downloader/add` (see `api.rs` for the full IMPL-VERIFY notes
//! against JD's `DebridLinkCom.java`).

pub(crate) mod api;
mod messages;
mod resolver;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
pub use native::DebridLinkResolver;

//! KrakenFiles resolver: the file page's download form, posted with a Cloudflare Turnstile
//! answer, and the metadata endpoint behind the site's own embed player for link checks.
//!
//! Measured on 2026-09-21 (RD-103-08): the file page is static HTML carrying everything the
//! download needs, there is no countdown, and the site's official API has no download
//! endpoint at all - so the route is the one JDownloader's `KrakenfilesCom` and pyLoad's
//! `KrakenfilesCom` drive, and nothing else.

/// Domains served by this hoster.
pub(crate) const HOSTERS: &[&str] = &["krakenfiles.com"];

mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
mod page;
mod resolver;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::KrakenfilesResolver;

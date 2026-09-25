//! DDownload resolver: documented metadata API plus cookie-backed premium transfer.

/// Domains served by this hoster (including the `ddl.to` short-link alias).
pub(crate) const HOSTERS: &[&str] = &["ddownload.com", "ddl.to"];

mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
mod page;
mod resolver;
mod session_trace;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::DdownloadResolver;

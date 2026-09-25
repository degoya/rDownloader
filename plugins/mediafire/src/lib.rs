//! MediaFire resolver (RD-103-06): the documented API for metadata and link checks, the
//! file page for the direct link.
//!
//! Public files only. `file/get_info` answers name, size, SHA-256, privacy and password
//! state for any public key without a session token; the direct link is on the file page in
//! the `downloadButton` anchor, new on every request. The premium route — `file/get_links`
//! with `link_type=direct_download` — needs an application registered in the person's own
//! MediaFire account, which an open-source plugin cannot ship a key for; the job file records
//! that decision. Folders are the sibling `plugins/mediafire-crawler`.
//!
//! Nothing is worked around: a captcha is handed to the host, a per-IP threshold is reported
//! as a wait, a file the site refuses is a failure with a code. A resolve that finds no direct
//! link fails with `mediafire.no_direct_link` and never hands the page address on, so nothing
//! downstream ever saves the HTML page as the file.

/// Domains served by this hoster.
pub(crate) const HOSTERS: &[&str] = &["mediafire.com", "mfi.re"];

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
pub use native::MediafireResolver;

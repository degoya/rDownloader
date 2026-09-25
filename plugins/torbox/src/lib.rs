//! TorBox resolver: the `[provider]` row every TorBox account hangs off, and the call that
//! turns a finished job's file into the address the bytes come from (RD-120-01).
//!
//! Narrow on purpose. `matches()` claims exactly one address shape -- TorBox's own
//! `requestdl` endpoint -- and nothing else, although TorBox will happily fetch a hoster link.
//! What it does with one is create a web-download job that takes minutes and answers with
//! files, and that is a remote job rather than a resolve: `plugins/torbox-jobs/` submits and
//! polls it. A resolver that claimed every address would be a resolver that took links away
//! from the plugins that can actually resolve them.
//!
//! The sibling plugins, because a manifest carries exactly one `plugin_type`:
//!
//! - `plugins/torbox-jobs/` runs magnets, torrent files, NZBs and web links as jobs at the
//!   provider, and hands back the `requestdl` addresses this plugin claims.
//! - `plugins/torbox-auth/` checks the pasted API key against the account.
//!
//! This plugin carries the `[provider]` row because only a resolver may (RD-106-04), and the
//! other two hang off it by claiming its slug.

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
pub use native::TorBoxResolver;

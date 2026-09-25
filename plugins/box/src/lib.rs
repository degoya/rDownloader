//! Box files and shared links (RD-120-05).
//!
//! One of three siblings, and the one the other two hang off. A manifest carries exactly one
//! `plugin_type`, and only a **resolver** manifest may carry a `[provider]` section — so the
//! `box` provider row, the account it creates and the two vault references that row owns all
//! live here. `box-oauth` fills them; `box-crawler` spends one of them. Neither of them can
//! exist without this plugin, which is the shape RD-106-04 decided for every cloud drive.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`api`] reads a Box item and turns Box's refusals into
//! codes, and [`resolver`] is the protocol logic both builds run. What the crawler sibling has
//! to answer identically — which hosts are Box's, how a shared link and a file inside it are
//! spelled, how a shared-link password reaches the `boxapi` header — lives in `box-common`
//! rather than being written twice. `guest` is the thin wrapper and exists only on `wasm32`.

mod api;
mod messages;
mod resolver;
mod target;

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
pub use native::BoxResolver;

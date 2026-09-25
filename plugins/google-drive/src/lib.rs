//! Google Drive files, shared links and Workspace exports (RD-106-04).
//!
//! One of three siblings, and the one the other two hang off. A manifest carries exactly one
//! `plugin_type`, and only a **resolver** manifest may carry a `[provider]` section — so the
//! `google_drive` provider row, the account it creates and the vault reference that row owns
//! all live here. `google-drive-oauth` fills that reference; `google-drive-crawler` spends it.
//! Neither of them can exist without this plugin, which is why "a cloud drive is two plugins"
//! turned out to be three.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`api`] reads what Drive answered, and [`resolver`] is
//! the protocol logic both builds run. What the crawler sibling has to answer identically —
//! which hosts are Google's, what an id may look like, what a Workspace document downloads as —
//! lives in `google-drive-common` rather than being written twice.
//! `guest` is the thin wrapper and exists only on `wasm32`.

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
pub use native::GoogleDriveResolver;

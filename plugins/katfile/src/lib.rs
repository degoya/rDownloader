//! KatFile resolver: documented XFS metadata API plus cookie-backed premium transfer.
//!
//! KatFile is an XFileSharing Pro (XFS) installation, the same hosting engine `plugins/ddownload`
//! runs; this plugin is built on the shared `xfs-common` crate extracted from ddownload in Task
//! 11, cloning ddownload's dual API-key/cookie mechanism exactly (see `native/api.rs`'s module doc
//! for the full IMPL-VERIFY record against JD's `KatfileCom.java` and its `XFileSharingProBasic`
//! base class).

/// Domains served by this hoster, `katfile.biz` (the current live main domain) first — mirrors
/// JD's `KatfileCom.getPluginDomains()` (rev 53112); see `native/api.rs`'s module doc.
pub(crate) const HOSTERS: &[&str] = &[
    "katfile.biz",
    "katfile.space",
    "katfile.ws",
    "katfile.vip",
    "katfile.online",
    "katfile.cloud",
    "katfile.com",
];

mod account;
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
pub use native::KatfileResolver;

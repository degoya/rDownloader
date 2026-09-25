//! Pixeldrain resolver: the provider's own public JSON API, account optional (RD-120-07,
//! RD-120-38).
//!
//! The route is the one measured on 2026-09-22 and recorded in
//! `docs/roadmap/jobs/120-07-pixeldrain.md`: `GET /api/file/{id}/info` for the metadata,
//! `GET /api/file/{id}` for the bytes with byte-range support, and stable machine-readable
//! refusals whose `value` field is the code this project translates. `robots.txt` on that host
//! reads `# Go ahead robots, do your worst`, so nothing here works around a stated wish.
//!
//! Two properties shape everything below:
//!
//! - **The download address does not expire.** `https://pixeldrain.com/api/file/{id}` is the
//!   file identifier and nothing else -- no signature, no deadline, no session. A job may sit an
//!   hour in the queue and the address it carries still works when its turn comes, so this
//!   plugin hands one out and does not have to re-resolve per attempt the way a provider that
//!   mints short-lived links does. It re-reads the metadata on every call anyway, which is what
//!   makes a file deleted in the meantime an honest refusal rather than a 404 mid-transfer.
//! - **The limits are the provider's, and they are reported rather than worked around.**
//!   Pixeldrain allows hotlinking only when the uploader or the downloader holds a premium
//!   subscription and otherwise bounds an IP's transfer volume, its concurrent downloads and its
//!   daily allowance. Each of those has a stable code here and a translation in all four
//!   languages. None of them is retried past, and none is dodged by asking for another address.
//!
//! The sibling package, because a manifest carries exactly one `plugin_type`:
//!
//! - `plugins/pixeldrain-crawler/` turns a `/l/{id}` list address into the file links behind it,
//!   which this resolver then handles one by one.
//!
//! There is no `pixeldrain-auth`: an API key is pasted, not signed in for. Pixeldrain
//! authenticates it as an HTTP Basic *password* under an empty user name. A plugin never holds
//! its credential, so the requests here write `{{basic:pixeldrain_api_key}}` and the host builds
//! the pair -- with an empty name, which this provider's row allows because it does not require
//! one -- and the download engine attaches the same pair to the transfer because the manifest
//! declares `transfer_auth = "basic"` (RD-120-38). Until then the row said `credentials =
//! "none"`, since nothing could send such a key. Without an account everything runs as before.

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
pub use native::PixeldrainResolver;

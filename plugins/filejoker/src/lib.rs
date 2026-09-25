//! FileJoker resolver: cookie-backed premium transfer only (XFileSharing-clone HTML flow). Unlike
//! `ddownload`/`katfile`, the registry defines no secret for this provider (`CredentialKind::Cookies`,
//! `secret_reference: None`) — see `native/api.rs`'s module doc for why no API-key path exists here
//! and for the full IMPL-VERIFY record (no JD reference exists; verified against the live site and
//! two independent community references).

/// Domain served by this hoster. Used only by `guest.rs`'s `hosters()` (the WIT `Guest` trait has
/// no default to fall back to, unlike the native `Resolver` trait — see `native.rs`'s module doc),
/// hence unused under a native (non-`wasm32`) build.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const HOSTERS: &[&str] = &["filejoker.net"];

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
pub use native::FilejokerResolver;

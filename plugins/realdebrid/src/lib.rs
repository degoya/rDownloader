//! Real-Debrid resolver: Bearer-authenticated JSON API (`api.real-debrid.com/rest/1.0`),
//! multihoster, signed in through the OAuth2 device flow rather than through a pasted key.
//!
//! `matches()` accepts any http(s) address the way the other multihosters do, because what a
//! multihoster covers is decided by the account's catalogue rather than by the address; the
//! catalogue itself comes from `hosts/domains`. `resolve()` posts the link to `unrestrict/link`
//! and hands back the `download` address it answers with.
//!
//! The sibling plugins, because a manifest carries exactly one `plugin_type`:
//!
//! - `plugins/realdebrid-auth/` signs the account in (`world oauth-plugin`, device entrance)
//!   and writes the access token into the slot this plugin reads.
//! - There is deliberately **no** crawler sibling. A Real-Debrid torrent is not a folder that
//!   can be listed in one call: it has to be uploaded, waited for, and have its files chosen
//!   before any address exists at all. `docs/roadmap/jobs/106-03-real-debrid.md` records why
//!   that makes it a persistent remote job rather than a `crawl`.

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
pub use native::RealDebridResolver;

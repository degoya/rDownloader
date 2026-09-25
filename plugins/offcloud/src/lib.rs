//! Offcloud resolver: Bearer-authenticated JSON API (`offcloud.com/api`), multihoster, with a
//! typed API key (RD-120-02).
//!
//! `matches()` accepts any http(s) address the way the other multihosters do, because what a
//! multihoster covers is decided by the account's catalogue rather than by the address; the
//! catalogue itself comes from `GET /api/sites`. `resolve()` posts the link to `/api/instant`
//! and hands back the address it answers with — **and asks again every time**, which is how a
//! short-lived Offcloud address is renewed: nothing is cached here, so a queue that comes back
//! to a link an hour later gets a link that is an hour younger rather than one that has died.
//!
//! The sibling package, because a manifest carries exactly one `plugin_type`:
//!
//! - `plugins/offcloud-cloud/` turns a magnet or an address into a persistent job in the
//!   account's Offcloud cloud, and hands back what it finished for this resolver's queue.
//!
//! There is deliberately **no** `offcloud-auth`. Offcloud's published API signs in with a key
//! the person copies from their account page; the second entrance it documents, `POST
//! /api/login` with an address and a password, is called obsolete by the provider's own
//! documentation. A sign-in plugin around it would put a password in the vault to obtain a key
//! the person can paste in one step, which is more credential in more places for nothing.
//! `docs/roadmap/jobs/120-02-offcloud.md` records the decision and what it leaves open.

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
pub use native::OffcloudResolver;

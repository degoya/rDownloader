//! Put.io files (RD-120-03).
//!
//! One of three siblings, and the one the other two hang off. A manifest carries exactly one
//! `plugin_type`, and only a **resolver** manifest may carry a `[provider]` section — so the
//! `putio` provider row, the account it creates and the two vault references that row owns all
//! live here. `putio-oauth` fills the token reference; `putio-transfers` spends it. Neither of
//! them can exist without this plugin.
//!
//! What it resolves is deliberately narrow. Put.io is a hoster for its own storage, not a
//! multihoster: it does not unrestrict anybody else's links, so this plugin claims Put.io's own
//! file addresses and nothing else. The addresses it claims are the ones its remote-job sibling
//! writes into the LinkGrabber, `https://api.put.io/v2/files/<id>/download`, and the one a
//! person copies out of the web interface.
//!
//! The resolve itself is one request and no short-lived address. `GET /v2/files/<id>/url`
//! exists and answers with a signed storage address that expires; this plugin deliberately
//! never asks for one. It answers with the stable per-file address instead, the host attaches
//! the account's token to it because the `putio` provider row says it may, and Put.io mints the
//! expiring address at the moment the bytes are fetched. The reasoning is in
//! `putio_common::address`.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`api`] reads what Put.io answered and classifies a refusal, and [`resolver`] is the
//! protocol logic both builds run. What the siblings have to answer identically lives in
//! `putio-common`. `guest` is the thin wrapper and exists only on `wasm32`.

mod api;
mod messages;
mod resolver;

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
pub use native::PutioResolver;

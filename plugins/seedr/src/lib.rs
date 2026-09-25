//! Seedr files (RD-120-04).
//!
//! One of two siblings, and the one the other hangs off. A manifest carries exactly one
//! `plugin_type`, and only a **resolver** manifest may carry a `[provider]` section — so the
//! `seedr` provider row, the account it creates and the vault reference that row owns all live
//! here. `seedr-jobs` spends what this plugin declares; it cannot exist without this one.
//!
//! What it resolves is deliberately narrow. Seedr is a hoster for its own storage, not a
//! multihoster: it does not unrestrict anybody else's links, so this plugin claims Seedr's own
//! file addresses and nothing else — the ones its remote-job sibling writes into the
//! LinkGrabber, `https://www.seedr.cc/rest/file/<id>`.
//!
//! Two things about Seedr's REST v1 shape what this plugin can do, and both are the provider's
//! rather than the plugin's:
//!
//! - **There is no per-file metadata call.** The documented `GET /rest/file/{id}` *is* the
//!   download, and the rest of the Files section is renaming, deleting and preview images. A
//!   resolver that wanted a file's name or size would have to fetch the file to learn them. So
//!   `resolve` makes no request at all and answers with the stable address it was given, and
//!   `check` answers `unknown` rather than inventing a status out of nothing. The names and
//!   sizes a person sees come from the folder listing the remote job read, which is where
//!   Seedr does state them.
//! - **Auth is HTTP Basic, and that reaches the transfer differently.** `check_account` sends
//!   `{{basic:seedr_password}}` and the host builds the blob; a *transfer* is fetched by the
//!   download engine, which runs no plugin code. So `manifest.toml` declares `transfer_auth =
//!   "basic"`, and the engine attaches the same pair to the bytes itself, to `www.seedr.cc`
//!   only (RD-120-38). `seedr_common::address` has the rest.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`api`] reads what Seedr answered and classifies a refusal, and [`resolver`] is the protocol
//! logic both builds run. What the siblings have to answer identically lives in `seedr-common`.
//! `guest` is the thin wrapper and exists only on `wasm32`.

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
pub use native::SeedrResolver;

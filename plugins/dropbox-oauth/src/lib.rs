//! Signing a Dropbox account in (RD-106-06).
//!
//! One of three siblings. This one holds no Dropbox file logic at all: it obtains a token and
//! hands it to the vault, and `plugins/dropbox/` and `plugins/dropbox-crawler/` spend it
//! without ever seeing it. The `dropbox` provider row it fills belongs to the resolver,
//! because only a resolver manifest may declare one.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a credential.** The tokens an exchange produces go back through
//!   `store-oauth-token`, which writes them where the account's provider keeps them. There is
//!   no call that reads one back. Later requests reach the stored value only as the template
//!   `{{secret:dropbox_access_token}}`, which the host expands on the way out.
//! - **You do not choose where the person is sent.** The address `begin` returns must be on a
//!   domain this plugin's manifest declares. The host refuses anything else rather than letting
//!   a signed, installed plugin put a sign-in page of its own choosing in front of somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call. What has to
//!   survive from `begin` to `poll` — the PKCE verifier — travels in `flow-state`, which the
//!   host stores verbatim, never shows, and never serialises out of the API.
//!
//! The layout follows from the third point plus one practical concern: everything that can be
//! tested without a WebAssembly toolchain lives outside the component. `cargo test` runs
//! [`flow`] on the host target, and `plugin-common` the shared [`pkce`]; `guest` exists
//! only on `wasm32`.

pub mod flow;

/// PKCE and the JSON reader, shared with the other OAuth plugins.
///
/// Re-exported rather than imported at each use site, so the paths below still read
/// `pkce::challenge` and a reader can see where the derivation lives.
pub use plugin_common::pkce;

#[cfg(target_arch = "wasm32")]
mod guest;

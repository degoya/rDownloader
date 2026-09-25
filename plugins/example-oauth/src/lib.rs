//! The reference OAuth provider (RD-105-01): authorization code with PKCE, and the renewal
//! that outlives it.
//!
//! It exists to be driven by the contract tests in `crates/rd-plugin-ext/tests/oauth_contract.rs`,
//! which stand in for the authorization server. It is the same code the `oauth` SDK template
//! scaffolds, pointed at a host nobody can reach — shipping a usable one would mean shipping a
//! plugin that signs people in somewhere.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a credential.** The tokens an exchange produces go back through
//!   `store-oauth-token`, which writes them where the account's provider keeps them. There is
//!   no call that reads one back. Later requests reach the stored value only as the template
//!   `{{secret:<reference>}}`, which the host expands on the way out.
//! - **You do not choose where the person is sent.** The address `begin` returns must be on a
//!   domain this plugin's manifest declares. The host refuses anything else rather than
//!   letting a signed, installed plugin put a sign-in page of its own choosing in front of
//!   somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call. What has to
//!   survive from `begin` to `poll` — the PKCE verifier — travels in `flow-state`, which the
//!   host stores verbatim, never shows, and never serialises out of the API.
//!
//! The layout follows from the third point plus one practical concern: everything that can be
//! tested without a WebAssembly toolchain lives outside the component. `cargo test` in a fresh
//! scaffold runs [`pkce`] and [`flow`] on the host target; `guest` exists only on `wasm32`.

pub mod flow;

/// PKCE and the JSON reader, shared with the other OAuth plugins.
///
/// Re-exported rather than imported at each use site, so the paths below still read
/// `pkce::challenge` and a reader can see where the derivation lives.
pub use plugin_common::pkce;

#[cfg(target_arch = "wasm32")]
mod guest;

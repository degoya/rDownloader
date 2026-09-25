//! Signing a Box account in (RD-120-05).
//!
//! One of three siblings. This one holds no Box file logic at all: it obtains a token and hands
//! it to the vault, and `plugins/box/` and `plugins/box-crawler/` spend it without ever seeing
//! it. The `box` provider row it fills belongs to the resolver, because only a resolver
//! manifest may declare one.
//!
//! Box is the cloud drive of the four where the person's own **client secret** has to survive
//! the sign-in, and that shapes everything below. Box's OAuth 2.0 has one entrance — the
//! browser redirect — and its token endpoint requires `client_secret` on both the exchange and
//! every renewal; there is no public-client variant and no PKCE to stand in for it. So the
//! account holds two credentials at once (RD-106-03): the secret the person registered, in the
//! slot they fill, and the access token this plugin obtains, in the slot marked
//! `filled_by = "flow"`. Writing the second over the first would destroy the value every later
//! renewal needs.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a credential.** The tokens an exchange produces go back through
//!   `store-oauth-token`, which writes them where the account's provider keeps them. There is
//!   no call that reads one back. The client secret reaches the token endpoint only as the
//!   template `{{secret:box_client_secret}}`, which the host expands on the way out and only
//!   towards the hosts that slot names.
//! - **You do not choose where the person is sent.** The address `begin` returns must be on a
//!   domain this plugin's manifest declares. The host refuses anything else rather than letting
//!   a signed, installed plugin put a sign-in page of its own choosing in front of somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call. What has to
//!   survive from `begin` to `poll` travels in `flow-state`, which the host stores verbatim,
//!   never shows, and never serialises out of the API.
//!
//! The layout follows from the third point plus one practical concern: everything that can be
//! tested without a WebAssembly toolchain lives outside the component. `cargo test` runs
//! [`flow`] on the host target, and `plugin-common` the shared [`pkce`]; `guest` exists only on
//! `wasm32`.

pub mod flow;

/// The unguessable-value helper and the JSON reader, shared with the other OAuth plugins.
///
/// Re-exported rather than imported at each use site, so the paths below still read
/// `pkce::string_field` and a reader can see where the derivation lives. Box takes no PKCE
/// challenge — it has no public-client entrance — but `state` is still drawn the same way, from
/// the host's random source and from nothing else.
pub use plugin_common::pkce;

#[cfg(target_arch = "wasm32")]
mod guest;

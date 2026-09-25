//! Signing a Put.io account in (RD-120-03).
//!
//! One of three siblings. This one holds no Put.io logic at all: it obtains a token and hands
//! it to the vault, and `plugins/putio/` and `plugins/putio-transfers/` spend it without ever
//! seeing it. The `putio` provider row it fills belongs to the resolver, because only a
//! resolver manifest may declare one.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a credential.** The token an exchange produces goes back through
//!   `store-oauth-token`, which writes it where the account's provider keeps it. There is no
//!   call that reads one back, and the client secret the exchange needs is sent as the template
//!   `{{secret:putio_client_secret}}`, which the host expands on the way out.
//! - **You do not choose where the person is sent.** The address `begin` returns must be on a
//!   domain this plugin's manifest declares. The host refuses anything else rather than letting
//!   a signed, installed plugin put a sign-in page of its own choosing in front of somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call. What has to
//!   survive from `begin` to `poll` travels in `flow-state`, which the host stores verbatim,
//!   never shows and never serialises out of the API.
//!
//! **Put.io issues no refresh material and states no expiry.** Its token endpoint answers with
//! an `access_token` and nothing else, and the token stays valid until the person revokes it.
//! `refresh` is implemented all the same, as the ordinary `grant_type=refresh_token` exchange,
//! because the alternative is a plugin that would have to be edited the day Put.io changes its
//! mind. What follows from today's behaviour is that nothing ever calls it: the renewal sweep
//! reads only flows that carry both a refresh reference and an expiry, and a Put.io sign-in
//! carries neither.
//!
//! **PKCE is not used**, and that is worth stating rather than leaving to be noticed. Put.io's
//! token endpoint is a confidential-client exchange that authenticates with the client secret;
//! it publishes no `code_challenge` support, and sending one to an endpoint that ignores it
//! would be a proof nobody checks. What does protect the callback is `state`, drawn from the
//! host's random source and compared by the host before a word of the callback is believed.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! `cargo test` runs [`flow`] on the host target; `guest` exists only on `wasm32`.

pub mod flow;

/// The JSON reader and the percent-encoder, shared with the other OAuth plugins.
///
/// Re-exported rather than imported at each use site, so the paths below still read
/// `pkce::string_field` and a reader can see where it lives.
pub use plugin_common::pkce;

#[cfg(target_arch = "wasm32")]
mod guest;

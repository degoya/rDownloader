//! Signing a pCloud account in (RD-120-06).
//!
//! One of three siblings. This one holds no pCloud file logic at all: it obtains a token and
//! hands it to the vault, and `plugins/pcloud/` and `plugins/pcloud-crawler/` spend it without
//! ever seeing it. The `pcloud` provider row it fills belongs to the resolver, because only a
//! resolver manifest may declare one.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a credential.** The token an exchange produces goes back through
//!   `store-oauth-token`, which writes it where the account's provider keeps it. There is no
//!   call that reads one back. Later requests reach the stored value only as the template
//!   `{{secret:pcloud_access_token}}`, which the host expands on the way out.
//! - **You do not choose where the person is sent.** The address `begin` returns must be on a
//!   domain this plugin's manifest declares. The host refuses anything else rather than letting
//!   a signed, installed plugin put a sign-in page of its own choosing in front of somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call.
//!
//! # What pCloud's OAuth is, and what it is not
//!
//! pCloud offers the authorization code flow and nothing else: **no PKCE** — its
//! `oauth2_token` documents `client_id`, `client_secret` and `code`, and no challenge — and
//! **no refresh token**, because the access token it issues does not expire until it is
//! revoked. So this plugin is a confidential client with the person's own application secret,
//! and [`flow`] has four answers rather than a verifier to keep.
//!
//! That is also why `refresh` here is a refusal and not an oversight: there is no renewal
//! material to renew from. The host never asks for one either — its sweep selects flows that
//! have both refresh material and an expiry, and this one stores neither.
//!
//! # Which installation redeems the code
//!
//! pCloud's redirect states the account's data centre as `hostname` and `locationid`, but the
//! host hands `poll` the `code` alone — the WIT contract carries no room for the rest, and
//! widening it would make all 68 signed components stale for one provider's benefit. So the
//! code is redeemed at `api.pcloud.com` and, if that installation does not know it, at
//! `eapi.pcloud.com`. A code the other installation never issued cannot be spent there, so the
//! failed attempt costs one request and consumes nothing.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives outside the component. `cargo test` runs [`flow`] on the host
//! target, and `plugin-common` the shared [`pkce`]; `guest` exists only on `wasm32`.

pub mod flow;

/// The random-value and JSON helpers shared with the other OAuth plugins.
///
/// Re-exported rather than imported at each use site, so the paths below still read
/// `pkce::percent_encode` and a reader can see where they live. pCloud offers no PKCE, so the
/// challenge half of it is deliberately unused here.
pub use plugin_common::pkce;

#[cfg(target_arch = "wasm32")]
mod guest;

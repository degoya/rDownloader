//! Real-Debrid sign-in: OAuth2 device code, and the renewal that outlives it (RD-106-03).
//!
//! The sibling of `plugins/realdebrid/`, because a manifest carries exactly one `plugin_type`.
//! This one signs the account in; that one resolves links with what this produced.
//!
//! **Why this is an `oauth` plugin and not an `auth` one.** Real-Debrid hands out access tokens
//! that die in an hour. `world auth-plugin` can run a device code but has no `refresh`, so a
//! person signed in through it would be asked for a new code every hour, for ever.
//! `interface oauth` has `refresh` and, since RD-106-01, `device-begin`/`device-poll` beside
//! it — the exact combination this provider needs, and the combination
//! `docs/adr/0002-a-device-sign-in-that-can-be-renewed.md` was written for.
//!
//! **The client is the person's own, and nothing secret is compiled in (RD-150-09).**
//! Real-Debrid offers open-source applications a flow of their own: the device code is asked
//! for with a public client id (`X245A4XAIBGVM`) and `new_credentials=yes`, the person confirms
//! it on real-debrid.com/device, and the confirmed code is exchanged at
//! `/oauth/v2/device/credentials` for a client id and client secret that belong to that person
//! alone. Those two and the device code then buy the token at `/oauth/v2/token`, and those two
//! and the refresh material renew it. Rate limits and revocation hang off the personal pair, so
//! no installation shares a bucket with another, and no client secret is ever shipped.
//!
//! Until 1.4.2 this plugin ran against an application the person had to register themselves,
//! which nobody could be asked to do; 1.5.0 parked it behind the private API token, which stays
//! as the provider's second mode.
//!
//! **A renewal needs three stored values, so the host keeps parts.** The personal client id and
//! client secret go to the host through `store-flow-secret`, each kept as a named part of this
//! account's sign-in; the tokens through `store-oauth-token`. Every later request names them --
//! `{{secret:realdebrid_client_id}}`, `{{secret:realdebrid_client_secret}}` and the refresh
//! reference -- and the host expands each on the way out, towards `api.real-debrid.com` alone.
//! One joined value would not do: the host percent-encodes what it substitutes, so a composite
//! could never be split back apart.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a stored credential.** What an exchange produces is read once off the
//!   provider's answer and goes straight back through `store-oauth-token` or
//!   `store-flow-secret`; there is no call that reads one back. Later requests reach the stored
//!   value only as `{{secret:<reference>}}`, expanded on the way out.
//! - **You do not choose where the person is sent.** The `verification_url` must be on a domain
//!   this plugin's manifest declares, or the host refuses it — rather than letting a signed,
//!   installed plugin put a sign-in page of its own choosing in front of somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call. What has to
//!   survive from `device-begin` to `device-poll` — the device code — travels in `flow-state`,
//!   which the host stores verbatim, never shows, and never serialises out of the API.

pub mod flow;
pub mod form;

#[cfg(target_arch = "wasm32")]
mod guest;

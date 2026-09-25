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
//! **The application is the person's own, and nothing about it is compiled in.** Real-Debrid
//! runs its device flow against a registered application: a client id and a client secret that
//! belong to whoever registered them. This plugin ships neither. An OAuth client secret in an
//! open-source repository is not a secret — it would stand in the git history, in every signed
//! `.rdplug` and in every release artefact anybody downloads — and worse than the disclosure is
//! the sharing: Real-Debrid's rate limits are per application, so one shipped registration
//! would put every installation in the world into one bucket and let any of them exhaust it for
//! all the others. Registered per installation means own limits and own revocation.
//!
//! So the person registers an application at Real-Debrid and enters it with the account: the
//! client id as the username, the client secret as the credential. The plugin only ever names
//! them — `{{username}}` and `{{secret:realdebrid_client_secret}}` — and the host substitutes
//! the values on the way out, towards `api.real-debrid.com` and nowhere else. An account with
//! no registration is refused before a request is made, with a code that says what to do rather
//! than that something is missing.
//!
//! **Two credentials have to exist at once, so the provider declares two slots.** The client
//! secret is what the person typed; the access token is what the sign-in obtained. Writing the
//! second over the first would destroy the value every later renewal needs, which is exactly
//! what `store-oauth-token` used to do — so the manifest marks the token's slot
//! `filled_by = "flow"`, and the host keeps it beside the flow instead of on the account.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a credential.** What the exchange produces goes back through
//!   `store-oauth-token`; there is no call that reads one back. Later requests reach the stored
//!   value only as `{{secret:<reference>}}`, expanded on the way out.
//! - **You do not choose where the person is sent.** The `verification_url` must be on a domain
//!   this plugin's manifest declares, or the host refuses it — rather than letting a signed,
//!   installed plugin put a sign-in page of its own choosing in front of somebody.
//! - **You remember nothing.** A guest is instantiated fresh for every call. What has to
//!   survive from `device-begin` to `device-poll` — the device code — travels in `flow-state`,
//!   which the host stores verbatim, never shows, and never serialises out of the API.

pub mod flow;
pub mod json;

#[cfg(target_arch = "wasm32")]
mod guest;

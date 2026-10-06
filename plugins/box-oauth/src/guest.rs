//! The component: Box's authorization code grant, and the renewal that outlives it.
//!
//! Redirect only. Box has no device flow, so `device-begin` and `device-poll` answer with a
//! stable code rather than a trap — the world requires them to exist, and the manifest's
//! `oauth_flows = ["redirect"]` means the host never calls them anyway.
//!
//! And no PKCE. Box's authorization server accepts no `code_challenge`, and its token endpoint
//! requires a `client_secret` on every grant, so what proves an exchange is the same flow is
//! the secret the person registered rather than a verifier this plugin kept back: a
//! [`Client::Confidential`]. `state` is still drawn from the host's random source and compared
//! by the host, which is what keeps a callback naming a flow nobody started from matching
//! anything. The flow itself is `plugin-guest-oauth`'s; this file states what is Box's.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_oauth::redirect::{CLIENT_ID_MARKER, Client, Device, Provider};

use crate::flow;

/// The application **this installation** registered for itself in the Box Developer Console.
///
/// There is deliberately no client id and no client secret in this file, and it is worth saying
/// why rather than leaving it to be rediscovered. A pair compiled in here would sit in the git
/// history of a public repository, in every signed `.rdplug` and in every release artefact —
/// not revocable, not rotatable — and, far more practically, Box counts its API rate limits per
/// application, so one compiled-in registration would have every installation in the world
/// sharing one allowance. Registered per installation, each one has its own (RD-106-04, rule 8).
///
/// Box requires the redirect URI to match exactly, port included. Its authorization code is good
/// for thirty seconds; the ten minutes a flow may sit open bound the walk through its pages.
const PROVIDER: Provider = Provider {
    slug: "box_oauth",
    name: "box",
    authorize_endpoint: "https://account.box.com/api/oauth2/authorize",
    token_endpoint: "https://api.box.com/oauth2/token",
    client_id: CLIENT_ID_MARKER,
    // No `scope` parameter, and that is a decision rather than an omission. Box takes the
    // scopes from the application's own configuration when none is named, and narrowing them
    // from here would either repeat what the person already ticked in the Developer Console or
    // ask for something their application does not have — which Box refuses outright. Least
    // privilege lives where Box put it: the account's catalogue text says to tick the read
    // scope and nothing that writes.
    scope: None,
    authorize_extra: "",
    token_extra: &[],
    client: Client::Confidential {
        // The account's own credential: the client secret of the application the person
        // registered. Box is the one cloud drive of the four that leaves no choice here: its
        // OAuth 2.0 has no public-client entrance and accepts no PKCE, so the token endpoint
        // refuses every grant that arrives without a secret. That is why the `box` provider
        // carries two credential slots (RD-106-03): this one, which the person fills and which
        // every renewal still needs, and the access token, which this plugin writes into a slot
        // of its own — beside the client secret rather than over it.
        secret_reference: "box_client_secret",
        unregistered: "this account has no registered Box application to sign in with",
    },
    device: Device::BrowserOnly("box"),
    waiting: flow::WAITING,
    refusal_code: flow::refusal_code,
};

plugin_guest_oauth::redirect_plugin!(PROVIDER);

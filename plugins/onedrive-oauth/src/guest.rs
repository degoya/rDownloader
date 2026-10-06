//! The component: Microsoft's authorization code with PKCE, its device code, and the renewal
//! that outlives both.
//!
//! Both entrances, because Microsoft offers both and the manifest says so
//! (`oauth_flows = ["redirect", "device"]`). Whichever one a sign-in takes, it ends in the same
//! `store-oauth-token` call with the same refresh material, and `refresh` renews it the same
//! way. The flow itself is `plugin-guest-oauth`'s; this file states what is Microsoft's.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_oauth::redirect::{CLIENT_ID_MARKER, Client, Device, Provider};

use crate::flow;

/// The least this needs, and it is worth being exact about why it is not less. `Files.Read`
/// covers the account's own drive, but the whole reason this plugin exists — a sharing link,
/// which is also the only way into SharePoint content here — goes through `/shares/{id}`,
/// which Graph grants to `Files.Read.All` and not to `Files.Read`. Still read-only: it cannot
/// delete, cannot share and cannot write, which matters more than usual because the same
/// token is spent by two other plugins. `offline_access` is what makes a refresh token exist at
/// all; without it the person would be asked again every hour.
const SCOPE: &str = "Files.Read.All offline_access";

/// There is deliberately no client id in this file (RD-106-04, rule 8). A client id compiled
/// in here would sit in the git history of a public repository, in every signed `.rdplug` and
/// in every release artifact — and Microsoft's throttling is counted per application, so one
/// compiled-in registration would have every installation in the world sharing one budget.
/// Registered per installation, each one has its own, in its own tenant, revocable by its own
/// administrator. The account's username field holds it, and the host expands the marker.
///
/// Microsoft accepts an `http` loopback redirect for a public client, and this exact one has to
/// be entered under the application's *Mobile and desktop applications* platform.
const PROVIDER: Provider = Provider {
    slug: "onedrive_oauth",
    name: "microsoft",
    // The `common` tenant lets a personal Microsoft account and a work or school account sign
    // in through one address — the application has to be registered for both, which the
    // account's hint says.
    authorize_endpoint: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    token_endpoint: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    client_id: CLIENT_ID_MARKER,
    scope: Some(SCOPE),
    authorize_extra: "&response_mode=query",
    // Microsoft's token endpoint asks for the scope on the exchange and on a refresh, and
    // answers a narrower token when it is left out.
    token_extra: &[("scope", SCOPE)],
    client: Client::Public,
    // Microsoft's device code is typed at `https://microsoft.com/devicelogin`.
    device: Device::Endpoint("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"),
    waiting: flow::WAITING,
    refusal_code: flow::refusal_code,
};

plugin_guest_oauth::redirect_plugin!(PROVIDER);

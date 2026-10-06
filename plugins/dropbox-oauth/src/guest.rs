//! The component: Dropbox's authorization code with PKCE, and the renewal that outlives it.
//!
//! Redirect only. Dropbox has no device flow, so `device-begin` and `device-poll` answer with
//! a stable code rather than a trap — the world requires them to exist, and the manifest's
//! `oauth_flows = ["redirect"]` means the host never calls them anyway. The flow itself is
//! `plugin-guest-oauth`'s; this file states what is Dropbox's about it.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_oauth::redirect::{CLIENT_ID_MARKER, Client, Device, Provider};

use crate::flow;

/// The app **this installation** registered for itself in the Dropbox App Console, written
/// as the marker the host expands from the account (RD-106-04, rule 8).
///
/// There is deliberately no app key in this file, and it is worth saying why rather than
/// leaving it to be rediscovered. A key compiled in here would sit in the git history of a
/// public repository, in every signed `.rdplug` and in every release artifact — and, far more
/// practically, Dropbox rate-limits per app, so one compiled-in app would have every
/// installation in the world sharing one allowance. Registered per installation, each one has
/// its own.
///
/// An app key identifies the application to Dropbox, not the person to the application:
/// Dropbox publishes it in the address the person is sent to. So it is not a credential, it is
/// stored in the clear as the account's username, and the host will put it into the
/// authorization URL as well as into the two requests. An account without one is refused
/// with `oauth.client_not_configured`, before anybody is sent anywhere.
///
/// Dropbox requires the redirect URI to match exactly, port included, and allows plain `http`
/// for the loopback address only.
const PROVIDER: Provider = Provider {
    slug: "dropbox_oauth",
    name: "dropbox",
    authorize_endpoint: "https://www.dropbox.com/oauth2/authorize",
    token_endpoint: "https://api.dropboxapi.com/oauth2/token",
    client_id: CLIENT_ID_MARKER,
    // The least this needs: who the account is, what a file is, its bytes, and what a shared
    // link points at. Nothing that writes, deletes or shares — which matters more here than
    // usual, because the same token is spent by two other plugins. Every one of these has to be
    // ticked under *Permissions* in the App Console, or Dropbox refuses the sign-in.
    scope: Some("account_info.read files.metadata.read files.content.read sharing.read"),
    // Not decoration: without it Dropbox issues a short-lived access token and no refresh
    // material at all, and every download started more than four hours after a sign-in would
    // ask somebody to sign in again — its access tokens last four hours, and the renewal is
    // what a Dropbox account lives on.
    authorize_extra: "&token_access_type=offline",
    token_extra: &[],
    client: Client::Public,
    device: Device::BrowserOnly("dropbox"),
    waiting: flow::WAITING,
    refusal_code: flow::refusal_code,
};

plugin_guest_oauth::redirect_plugin!(PROVIDER);

//! The component: Google's authorization code with PKCE, and the renewal that outlives it.
//!
//! Redirect only. Google does run a device flow, but it is scoped to input-constrained devices
//! and does not grant the Drive scopes this needs, so `device-begin` and `device-poll` answer
//! with a stable code rather than a trap — the world requires them to exist, and the manifest's
//! `oauth_flows = ["redirect"]` means the host never calls them anyway. The flow itself is
//! `plugin-guest-oauth`'s; this file states what is Google's about it.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_oauth::redirect::{CLIENT_ID_MARKER, Client, Device, Provider};

use crate::flow;

/// The OAuth client **this installation** registered for itself, written as the marker the host
/// expands from the account (RD-106-04).
///
/// There is deliberately no client id in this file, and it is worth saying why rather than
/// leaving it to be rediscovered. A client id compiled in here would sit in the git history of
/// a public repository, in every signed `.rdplug` and in every release artifact — and, far more
/// practically, Google's quotas are counted per client, so one compiled-in project would have
/// every installation in the world sharing one allowance. Registered per installation, each one
/// has its own, and each one also decides its own client type rather than this project guessing
/// which type Google's token endpoint will accept.
///
/// A client id identifies the application to Google, not the person to the application: Google
/// publishes it in the address the person is sent to. So it is not a credential, it is stored
/// in the clear as the account's username, and the host will put it into the authorization URL
/// as well as into the two requests. An account without one is refused with
/// `oauth.client_not_configured`, before anybody is sent anywhere.
///
/// Google matches a loopback redirect on everything but the port, so one registration serves
/// every installation.
const PROVIDER: Provider = Provider {
    slug: "google_drive_oauth",
    name: "google",
    authorize_endpoint: "https://accounts.google.com/o/oauth2/v2/auth",
    token_endpoint: "https://oauth2.googleapis.com/token",
    client_id: CLIENT_ID_MARKER,
    // The least this needs: read what the account can see, and nothing else. `drive.readonly`
    // cannot delete, cannot share and cannot write — which matters more here than usual,
    // because the same token is spent by two other plugins.
    scope: Some("https://www.googleapis.com/auth/drive.readonly"),
    // `access_type=offline` and `prompt=consent` are not decoration: without the first Google
    // issues no refresh material at all, and without the second it issues it exactly once —
    // so an account signed in twice would come back the second time with nothing to renew
    // from, and the person would be asked again every hour for ever (Google's access tokens
    // last an hour; the renewal is what a Drive account lives on).
    authorize_extra: "&access_type=offline&prompt=consent&include_granted_scopes=true",
    token_extra: &[],
    client: Client::Public,
    device: Device::BrowserOnly("google drive"),
    waiting: flow::WAITING,
    refusal_code: flow::refusal_code,
};

plugin_guest_oauth::redirect_plugin!(PROVIDER);

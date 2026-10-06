//! The component: authorization code with PKCE, a device code, and the renewal that outlives
//! both.
//!
//! What the `oauth` SDK template scaffolds, stated as a [`Provider`] rather than written out:
//! the endpoints are on `oauth.example.invalid`, which resolves nowhere, and the contract tests
//! answer for it. The bindings and the flow come from `plugin-guest-oauth`
//! (`redirect.rs` reads as the template's `guest.rs` does), which a standalone template does
//! not have and generates for itself.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_oauth::redirect::{Client, Device, Provider};

use crate::flow;

/// What a provider states; everything else is the shared flow.
///
/// The client is the one you registered with the provider. A client id identifies the
/// application, not the person, so it is public by design. A client *secret* is not: a provider
/// that requires one takes it the way a renewal takes the refresh token — as the template
/// `{{secret:<reference>}}`, which the host expands on the way out and hands back to nobody
/// ([`Client::Confidential`]).
const PROVIDER: Provider = Provider {
    slug: "example_oauth",
    name: "the provider",
    // Where the person agrees. Must be on a domain `manifest.toml` declares.
    authorize_endpoint: "https://oauth.example.invalid/oauth/authorize",
    token_endpoint: "https://oauth.example.invalid/oauth/token",
    client_id: "example-oauth-client",
    // Ask for the least the plugin needs, and for the renewal scope — without it there is no
    // refresh material and the person is asked again every time the token ages out.
    scope: Some("offline_access"),
    authorize_extra: "",
    token_extra: &[],
    client: Client::Public,
    // The device entrance (RD-106-01). A provider that offers no device flow has no such
    // endpoint: `Device::BrowserOnly` answers both `device-*` calls with a `flow_unsupported`
    // refusal, and dropping `"device"` from `oauth_flows` in `manifest.toml` keeps the host
    // from calling them at all.
    device: Device::Endpoint("https://oauth.example.invalid/oauth/device/code"),
    waiting: flow::WAITING,
    refusal_code: flow::refusal_code,
};

plugin_guest_oauth::redirect_plugin!(PROVIDER);

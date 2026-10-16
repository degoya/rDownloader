//! The address rule behind a proxy (RD-1320-07).
//!
//! Without a proxy, [`crate::GuardedResolver`] hands the connector only addresses the rule
//! permits, so the checked address is the dialled one. Through an HTTP proxy that resolver
//! never runs: the proxy is handed the *name* and resolves it itself, and a name that answered
//! the check here with a public address can answer the proxy a moment later with one inside
//! the proxy's network (DNS rebinding). [`with_proxy`] closes that: the name is resolved and
//! checked here, at the moment the connection is made, and the proxy is asked for a tunnel to
//! exactly that address — `CONNECT <ip>:<port>` — for HTTPS and plain HTTP alike. Inside the
//! tunnel the request is the one a direct connection would carry: TLS with the name as SNI and
//! the certificate checked against the name, with the client's own roots and custom CA; plain
//! HTTP in origin-form with the name in `Host`.
//!
//! reqwest has no way to dial a proxy tunnel to an address it did not resolve: its connector
//! layers can neither read nor rewrite the destination, and every HTTP proxy is handed the
//! name. What reqwest does do is resolve the target itself, with the client's resolver, for a
//! `socks5://` proxy, and hand that proxy the address. So a guarded client behind an HTTP(S)
//! proxy is given the guarded resolver and, as its proxy, a bridge: a SOCKS5 listener on
//! loopback, private to that client by a random password, which turns each checked address
//! into a `CONNECT` to the real proxy. Every caller keeps its `reqwest::Client`.
//!
//! Plain HTTP goes through `CONNECT` too, not as an absolute-form request to the address with
//! the name in `Host`: a proxy has to replace that `Host` with the address (RFC 9112, 3.2.2),
//! and the server would answer for the wrong site. A proxy that permits `CONNECT` only to port
//! 443 refuses such a tunnel, and the request fails with [`crate::CODE_TUNNEL_REFUSED`] rather
//! than reaching the name unchecked.
//!
//! A SOCKS5 profile goes through the bridge too, and a `socks5h://` one the same way: the bridge
//! resolves the proxy's own name and hands the proxy the checked address. reqwest could do the
//! latter itself, but only by resolving the proxy's name with the client's resolver — the one
//! that judges targets — and a name let past it for being the proxy's let a stranger's
//! `http://proxy.lan:8080/` reach the proxy's host on any port.
//!
//! A profile may say that its proxy resolves names itself ([`NameResolution::AtProxy`]), for a
//! network where only the proxy can: the proxy is then handed the name, as before this rule,
//! and the name is judged only before the request ([`check_before_request`]).
//!
//! A request no proxy profile covers — a webhook, a Web Push message, the reconnect's IP check —
//! takes the proxy the environment names, through the same bridge and always with names resolved
//! here ([`EnvironmentRoute`], RD-1330-01). Behind any pinned proxy a name this machine cannot
//! resolve fails as [`NameUnresolved`], the stable code [`CODE_NAME_UNRESOLVED`].

use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::{ClientBuilder, Proxy};
use secrecy::ExposeSecret;
use url::Url;

use crate::{
    AddressPolicy, AddressRefused, GuardedResolver, HostLookup, ProxyCredentials, SystemLookup,
    TargetRefusal,
};

#[path = "pinned_proxy_bridge.rs"]
mod bridge;
#[path = "pinned_proxy_environment.rs"]
mod environment;
#[path = "pinned_proxy_unresolved.rs"]
mod unresolved;
#[path = "pinned_proxy_upstream.rs"]
mod upstream;

use bridge::Bridge;
pub use environment::{EnvironmentProxy, EnvironmentRoute};
pub use unresolved::{CODE_NAME_UNRESOLVED, NameUnresolved, name_unresolved, unresolved_failure};
use upstream::Upstream;

/// Where the target names of a guarded request are resolved: the one rule the client pool, the
/// plugin host and the hand-over fetch follow (RD-1320-07).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameResolution {
    /// Here, by the guarded resolver as the connection is made; a proxy is handed the checked
    /// address. Without a proxy, and behind one unless its profile says otherwise.
    Here,
    /// At the proxy, which the profile says resolves names itself: the name is judged only
    /// before the request, and one this machine cannot resolve is left to the proxy.
    AtProxy,
}

impl NameResolution {
    /// The resolution of a request through `proxy`, or through none.
    #[must_use]
    pub fn of(proxy: Option<&rd_core::ProxyProfile>) -> Self {
        Self::resolving_at_proxy(proxy.is_some_and(|proxy| proxy.resolves_names))
    }

    /// [`Self::AtProxy`] when `at_proxy`, which only a proxy profile can say.
    #[must_use]
    pub const fn resolving_at_proxy(at_proxy: bool) -> Self {
        if at_proxy { Self::AtProxy } else { Self::Here }
    }
}

/// Judges `target` before the request, where the connection does not (RD-150-03): a literal
/// address always, since it never reaches a resolver, and a name when the proxy resolves it
/// ([`NameResolution::AtProxy`]) — one this machine cannot resolve passes then, for the proxy.
/// A name resolved [`NameResolution::Here`] is the guarded resolver's to judge.
pub async fn check_before_request(
    policy: &AddressPolicy,
    lookup: &dyn HostLookup,
    target: &Url,
    resolution: NameResolution,
) -> Result<(), AddressRefused> {
    if crate::literal_address(target).is_none() && resolution == NameResolution::Here {
        return Ok(());
    }
    match crate::check_target(policy, lookup, target).await {
        Ok(_) | Err(TargetRefusal::Unresolved(_)) => Ok(()),
        Err(TargetRefusal::Refused(refused)) => Err(refused),
    }
}

/// Gives `builder` the proxy at `endpoint`. Under an address rule (`guard`) every target is
/// resolved and checked here when the connection is made and the proxy is handed the checked
/// address, never the name — unless `resolution` says the proxy resolves names itself.
/// `custom_ca_pem` is what an `https://` proxy's own certificate is checked with; the targets'
/// certificates are the client's business, as without a proxy.
pub fn with_proxy(
    builder: ClientBuilder,
    endpoint: &Url,
    credentials: Option<&ProxyCredentials>,
    guard: Option<&AddressPolicy>,
    resolution: NameResolution,
    custom_ca_pem: &[Vec<u8>],
) -> Result<ClientBuilder> {
    with_proxy_lookup(
        builder,
        endpoint,
        credentials,
        guard,
        resolution,
        custom_ca_pem,
        Arc::new(SystemLookup),
    )
}

/// [`with_proxy`] over `lookup`, which resolves the targets and the proxy's own name: a caller's
/// test makes a name answer with anything there, as this crate's own do (RD-1330-01).
pub fn with_proxy_lookup(
    builder: ClientBuilder,
    endpoint: &Url,
    credentials: Option<&ProxyCredentials>,
    guard: Option<&AddressPolicy>,
    resolution: NameResolution,
    custom_ca_pem: &[Vec<u8>],
    lookup: Arc<dyn HostLookup>,
) -> Result<ClientBuilder> {
    match guard {
        Some(policy) if resolution == NameResolution::Here => pinned(
            builder,
            policy,
            endpoint,
            credentials,
            custom_ca_pem,
            lookup,
        ),
        _ => Ok(builder.proxy(profile_proxy(endpoint, credentials)?)),
    }
}

/// The pinned half of [`with_proxy`], over any lookup; tests make a name answer with anything.
pub(crate) fn pinned(
    builder: ClientBuilder,
    policy: &AddressPolicy,
    endpoint: &Url,
    credentials: Option<&ProxyCredentials>,
    custom_ca_pem: &[Vec<u8>],
    lookup: Arc<dyn HostLookup>,
) -> Result<ClientBuilder> {
    // The bridge resolves the proxy's own name with the same lookup, but never through the
    // guarded resolver, which judges targets alone.
    let upstream = Upstream::new(endpoint, credentials, custom_ca_pem, Arc::clone(&lookup))?;
    let tickets = Arc::new(crate::tunnel_refusal::Tickets::default());
    let bridge = Bridge::start(upstream, policy.clone(), Arc::clone(&tickets))?;
    // A name that does not resolve here fails as one, never handed to the proxy instead.
    let targets = Arc::new(unresolved::ResolvedHere(lookup));
    Ok(builder
        .proxy(bridge.into_proxy())
        .connector_layer(crate::tunnel_refusal::TicketLayer(tickets))
        .dns_resolver(GuardedResolver::with_lookup(policy.clone(), targets)))
}

/// The proxy at `endpoint` as reqwest speaks to it, with the profile's credentials.
pub(crate) fn profile_proxy(
    endpoint: &Url,
    credentials: Option<&ProxyCredentials>,
) -> Result<Proxy> {
    let mut proxy = Proxy::all(endpoint.as_str()).context("create proxy")?;
    if let Some(credentials) = credentials {
        proxy = proxy.basic_auth(&credentials.username, credentials.password.expose_secret());
    }
    Ok(proxy)
}

#[cfg(test)]
#[path = "pinned_proxy_fixtures.rs"]
mod fixtures;

#[cfg(test)]
#[path = "pinned_proxy_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pinned_proxy_setting_tests.rs"]
mod setting_tests;

#[cfg(test)]
#[path = "pinned_proxy_environment_tests.rs"]
mod environment_tests;

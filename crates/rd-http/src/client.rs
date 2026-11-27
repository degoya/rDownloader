//! Where every reqwest client of the service starts (RD-1280-06).
//!
//! Each client used to start from `reqwest::Client::builder()` on its own, and the user agent,
//! the connect deadline and whether the environment's proxy variables apply drifted between
//! them. [`client_builder`] sets those three; a caller adds what is its own: the total and read
//! deadlines, the redirect policy, a guarded resolver, a custom CA, a profile's proxy.

use std::time::Duration;

/// How long opening a connection may take, unless the caller sets its own deadline.
pub const CLIENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// What a client is for, which decides whether the environment's proxy variables apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientPurpose {
    /// The service's own request to an outside endpoint (a tracker, a solver, a webhook, a
    /// plugin repository, a tool archive): `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` apply, as
    /// for any other program on the machine.
    Service,
    /// A request whose proxy a profile decides, or that must never leave through one (a
    /// download, a site rule, the local control API, an identity provider): the environment's
    /// proxy variables are ignored, and the caller adds the profile's proxy itself.
    Direct,
}

/// A client builder with the service's user agent, [`CLIENT_CONNECT_TIMEOUT`] and the proxy
/// rule of `purpose`.
///
/// On a machine whose platform store holds no CA certificate the builder trusts only the roots
/// a caller adds — a custom CA — and none without one (RD-1490-13): `build()` would otherwise
/// fail there, and the callers that fall back to `Client::default()` panicked at the start.
pub fn client_builder(purpose: ClientPurpose) -> reqwest::ClientBuilder {
    let builder = with_platform_trust(
        reqwest::Client::builder()
            .user_agent(rd_core::user_agent!())
            .connect_timeout(CLIENT_CONNECT_TIMEOUT),
        crate::platform_trust_problem().is_none(),
    );
    match purpose {
        ClientPurpose::Service => builder,
        ClientPurpose::Direct => builder.no_proxy(),
    }
}

/// `builder` as it is where the platform store can verify a server, else trusting only the
/// roots added to it afterwards.
fn with_platform_trust(builder: reqwest::ClientBuilder, available: bool) -> reqwest::ClientBuilder {
    if available {
        builder
    } else {
        builder.tls_certs_only(Vec::<reqwest::Certificate>::new())
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

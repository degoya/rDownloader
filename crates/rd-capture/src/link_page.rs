//! The page that links the agent to a service without a shell (RD-1390-02).
//!
//! It is served on the Click'n'Load port, because that is the one listener the agent has, and
//! every page in the browser can reach that port: Click'n'Load exists precisely so a hoster page
//! can call it. For this page that reach is the danger, and everything here answers it.
//!
//! - The page lives under a secret path, `/link/<secret>`: 32 random bytes, new for every agent
//!   process, compared in constant time. A wrong or missing secret is a plain 404, the same as a
//!   route that does not exist, so a page that guesses learns nothing.
//! - The `Host` header has to name this port on a loopback name. A foreign name is DNS
//!   rebinding: a page of `evil.example` whose name now resolves to 127.0.0.1.
//! - Storing takes a `POST` whose `Origin` is this port on a loopback name; nothing answers a
//!   CORS preflight and no answer carries a CORS header, so another origin can neither read an
//!   answer nor send JSON.
//! - No answer carries the token, every answer says `no-store`, `no-referrer` and a CSP that
//!   allows nothing but the page's own inline code and requests to itself.
//!
//! Nothing is stored before the service accepted the token ([`verify`]); storing is
//! `config::save`, the same as `configure`.

use std::fmt::Write as _;

use axum::http::HeaderValue;
use rand::Rng;
use subtle::ConstantTimeEq as _;

mod routes;
mod verify;

pub(crate) use routes::{LinkState, Store, router};
pub(crate) use verify::{LinkRefusal, LinkRequest, Linked, verify_and_save};

/// The names a browser on this machine reaches the agent under.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "[::1]", "localhost"];

/// The secret in the page's path: 32 random bytes as 64 hex characters, one per agent process.
pub(crate) struct LinkSecret(String);

impl LinkSecret {
    /// A new secret from the operating system's random source.
    pub(crate) fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let mut text = String::with_capacity(64);
        for byte in bytes {
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Never the value: a secret in a log line is a secret anybody with the log has.
impl std::fmt::Debug for LinkSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LinkSecret(..)")
    }
}

/// Whether a request's `Host` names this port on a loopback name, in any case.
pub(crate) fn host_allowed(host: Option<&HeaderValue>, port: u16) -> bool {
    host.and_then(|value| value.to_str().ok())
        .is_some_and(|authority| is_loopback_authority(authority, port))
}

/// Whether a request's `Origin` is a page of this port on a loopback name. A request without
/// one, and the opaque `null` of a sandboxed frame or a file, is not.
pub(crate) fn origin_allowed(origin: Option<&HeaderValue>, port: u16) -> bool {
    origin
        .and_then(|value| value.to_str().ok())
        .and_then(|origin| origin.strip_prefix("http://"))
        .is_some_and(|authority| is_loopback_authority(authority, port))
}

/// Whether `given` is the secret, compared in constant time.
pub(crate) fn secret_matches(given: &str, secret: &LinkSecret) -> bool {
    bool::from(given.as_bytes().ct_eq(secret.as_str().as_bytes()))
}

/// `<loopback name>:<port>`, exactly: no other port, no path, no user.
fn is_loopback_authority(authority: &str, port: u16) -> bool {
    let Some((name, given)) = authority.rsplit_once(':') else {
        return false;
    };
    given == port.to_string()
        && LOOPBACK_HOSTS
            .iter()
            .any(|host| name.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod route_tests;
// The fixture service and the recording store serve `link_phase`'s tests as well.
#[cfg(test)]
pub(crate) mod tests;

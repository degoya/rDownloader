//! Passkeys: which origin a ceremony is bound to, and the state that lives between its halves.
//!
//! ## Why this is the one place a dependency was taken
//!
//! The CIDR matcher, the TOTP generator and the base32 codec in this crate are hand-written,
//! because each is small and each has *published* reference vectors — RFC 6238 and RFC 4648
//! print the expected outputs, so a test can prove conformance rather than self-consistency.
//! WebAuthn has no equivalent. Its verification procedure is a numbered list in a W3C
//! document with attacker-relevant steps (challenge binding, origin and RP-id hash checks,
//! the user-verification flag, algorithm selection) and no canonical vector table to check
//! against. Testing an implementation of it against fixtures one generated oneself proves the
//! code agrees with itself, which is exactly the property a subtle authentication bypass also
//! has. So the ceremony is `webauthn-rs`'s, and this module is only the part that is ours:
//! deciding the relying party and holding the in-flight state.
//!
//! ## Where the relying party comes from
//!
//! WebAuthn binds a credential to an origin, and that binding is the whole of its phishing
//! resistance. So the origin cannot be taken from whatever the request claims — a header the
//! caller controls would let the caller choose what the credential protects.
//!
//! The configured external URL ([`ProxyConfig::origin`]) is therefore the source of truth.
//! The one exception is `localhost`, which is what a plain local install is reached at before
//! anybody configures anything: a browser will not send `Origin: http://localhost:…` from
//! another site's page, and a name that resolves elsewhere (`localhost.example.com`) does not
//! match. An IP literal is refused rather than tried, because an RP id must be a domain and
//! browsers reject an address — refusing here turns that into an explanation instead of an
//! opaque failure inside the authenticator.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use rand::RngCore;
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

use crate::ProxyConfig;

/// How long a started ceremony stays answerable.
///
/// Long enough to find the phone, short enough that an abandoned challenge is not a lasting
/// piece of server state. The authenticator's own timeout is shorter in practice.
pub const CEREMONY_TTL: Duration = Duration::from_secs(300);

/// How many ceremonies may be in flight at once.
///
/// The authentication half is reachable without a session — it has to be, it is how you sign
/// in — so it allocates server state for an anonymous caller. The cap is what stops that from
/// being a way to grow the process. A legitimate user signing in is one entry, and the ceiling
/// is generous enough that no real usage reaches it.
const MAX_IN_FLIGHT: usize = 64;

/// How many ceremonies one address may hold at once.
///
/// The ceiling used to be global, and the entry dropped when it was reached was the *oldest*
/// — so somebody looping on the challenge endpoint evicted the legitimate user's challenge
/// before they could touch their authenticator, and the limit chosen to prevent a denial of
/// service was one. Now an address can only ever push out its own entries: this per-address
/// cap is reached first, and when the global one is reached as well it is the address holding
/// the most entries that gives one up, never the address holding one.
const MAX_PER_OWNER: usize = 8;

/// The name shown in the authenticator's account picker.
pub const RELYING_PARTY_NAME: &str = "rDownloader";

/// Why a ceremony cannot be started here.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RelyingPartyError {
    /// Nothing says what this installation is called from outside.
    #[error(
        "Passkeys need to know the address this installation is reached at. Set the external \
         URL under the reverse-proxy settings, or reach the service at http://localhost."
    )]
    OriginUnknown,
    /// The origin is an IP address, which cannot be a relying party id.
    #[error(
        "Passkeys cannot be bound to an IP address. Use a host name — http://localhost for a \
         local install, or a domain behind the reverse proxy."
    )]
    OriginIsAnAddress,
    /// The origin parsed but `webauthn-rs` refused it.
    #[error("The external URL cannot be used as a passkey origin: {0}")]
    OriginUnusable(&'static str),
}

/// Builds the relying party this installation's passkeys belong to.
///
/// `request_origin` is the `Origin` the caller's browser sent. It is consulted only when no
/// external URL is configured, and then only if it is loopback — see the module note.
pub fn relying_party(
    proxy: &ProxyConfig,
    request_origin: Option<&str>,
) -> Result<Webauthn, RelyingPartyError> {
    let origin = match proxy.origin() {
        Some(configured) => configured.to_owned(),
        None => request_origin
            .filter(|candidate| is_loopback_origin(candidate))
            .ok_or(RelyingPartyError::OriginUnknown)?
            .to_owned(),
    };
    let url = Url::parse(&origin).map_err(|_| RelyingPartyError::OriginUnusable("not a URL"))?;
    let host = url
        .host_str()
        .ok_or(RelyingPartyError::OriginUnusable("no host"))?
        .to_owned();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(RelyingPartyError::OriginIsAnAddress);
    }
    WebauthnBuilder::new(&host, &url)
        .and_then(|builder| builder.rp_name(RELYING_PARTY_NAME).build())
        .map_err(|_| RelyingPartyError::OriginUnusable("rejected by the relying party builder"))
}

/// Whether an origin names the loopback interface by a name a browser will honour.
///
/// Only `localhost` and its subdomains, which the URL spec treats as potentially trustworthy.
/// An IP literal is deliberately not accepted: it would pass this check and then fail inside
/// the browser, which is a worse way to learn the same thing.
fn is_loopback_origin(origin: &str) -> bool {
    Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "localhost" || host.ends_with(".localhost"))
}

/// The handle a client gives back to finish the ceremony it started.
pub type CeremonyId = String;

/// In-flight ceremony state, keyed by a handle the client echoes back.
///
/// In memory on purpose, like [`crate::throttle`]: `webauthn-rs` does not serialise this state
/// without an explicitly dangerous feature flag, because a challenge that outlives the process
/// is a challenge that can be replayed into it. A restart mid-sign-in costs one retry.
pub struct CeremonyStore<T> {
    entries: Mutex<HashMap<CeremonyId, Entry<T>>>,
}

struct Entry<T> {
    /// Which address started it. Only used to keep one caller's flood off another's entry.
    owner: IpAddr,
    state: T,
    started: Instant,
}

impl<T> Default for CeremonyStore<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> CeremonyStore<T> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Files a started ceremony and returns the handle that finishes it.
    ///
    /// `owner` is the address the request was attributed to. Nothing is refused — refusing
    /// would let anyone who can reach the login page stop everyone else from using it — but a
    /// caller only ever evicts entries of the address that is over its share, so a flood from
    /// one address cannot take the challenge the person at another address is answering.
    pub fn insert(&self, owner: IpAddr, state: T) -> CeremonyId {
        let id = random_handle();
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.started.elapsed() < CEREMONY_TTL);
        while entries
            .values()
            .filter(|entry| entry.owner == owner)
            .count()
            >= MAX_PER_OWNER
        {
            let Some(oldest) = oldest_of(&entries, owner) else {
                break;
            };
            entries.remove(&oldest);
        }
        while entries.len() >= MAX_IN_FLIGHT {
            // The global ceiling is reached only when several addresses are each within their
            // own share, so the one holding the most is the one asked to give an entry up.
            let Some(victim) = busiest_owner(&entries).and_then(|owner| oldest_of(&entries, owner))
            else {
                break;
            };
            entries.remove(&victim);
        }
        entries.insert(
            id.clone(),
            Entry {
                owner,
                state,
                started: Instant::now(),
            },
        );
        id
    }

    /// Claims a ceremony. Removing rather than reading is what makes a challenge single-use.
    pub fn take(&self, id: &str) -> Option<T> {
        let entry = self.lock().remove(id)?;
        (entry.started.elapsed() < CEREMONY_TTL).then_some(entry.state)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<CeremonyId, Entry<T>>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The handle of `owner`'s longest-standing ceremony.
fn oldest_of<T>(entries: &HashMap<CeremonyId, Entry<T>>, owner: IpAddr) -> Option<CeremonyId> {
    entries
        .iter()
        .filter(|(_, entry)| entry.owner == owner)
        .min_by_key(|(_, entry)| entry.started)
        .map(|(key, _)| key.clone())
}

/// The address holding the most ceremonies. The one that can afford to lose one.
fn busiest_owner<T>(entries: &HashMap<CeremonyId, Entry<T>>) -> Option<IpAddr> {
    let mut counts: HashMap<IpAddr, usize> = HashMap::new();
    for entry in entries.values() {
        *counts.entry(entry.owner).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(owner, _)| owner)
}

/// 128 bits of handle. Not a secret — the signature is what authenticates — but it must not
/// be guessable, or one caller could claim another's in-flight ceremony.
fn random_handle() -> CeremonyId {
    let mut bytes = [0_u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_with(origin: Option<&str>) -> ProxyConfig {
        ProxyConfig::parse(&[], origin, crate::CookieSecurity::Auto)
            .expect("the test proxy configuration is valid")
    }

    #[test]
    fn the_configured_external_url_decides_the_relying_party() {
        let webauthn = relying_party(&proxy_with(Some("https://dl.example.com")), None)
            .expect("a configured https origin is usable");
        assert_eq!(
            webauthn.get_allowed_origins()[0].as_str(),
            "https://dl.example.com/"
        );
    }

    /// The point of the whole module: a caller-supplied header must not be able to say what a
    /// credential protects, or the phishing resistance WebAuthn exists for is gone.
    #[test]
    fn a_request_origin_cannot_override_the_configured_one() {
        let webauthn = relying_party(
            &proxy_with(Some("https://dl.example.com")),
            Some("https://evil.example.net"),
        )
        .expect("a configured origin is usable");
        let origins = webauthn.get_allowed_origins();
        assert!(
            origins
                .iter()
                .all(|origin| origin.as_str().contains("dl.example.com")),
            "the request origin leaked into the relying party: {origins:?}"
        );
    }

    #[test]
    fn an_unconfigured_install_is_told_what_is_missing_rather_than_guessed_for() {
        assert_eq!(
            relying_party(&proxy_with(None), Some("https://dl.example.com"))
                .expect_err("an unconfigured install has no origin to bind to"),
            RelyingPartyError::OriginUnknown
        );
        assert_eq!(
            relying_party(&proxy_with(None), None)
                .expect_err("nothing says what this install is called"),
            RelyingPartyError::OriginUnknown
        );
    }

    /// A plain local install has to work before anybody configures anything.
    #[test]
    fn localhost_is_accepted_from_the_request_when_nothing_is_configured() {
        assert!(relying_party(&proxy_with(None), Some("http://localhost:8710")).is_ok());
    }

    /// `localhost.evil.example` resolves wherever its owner points it. It is not loopback.
    #[test]
    fn a_name_that_merely_starts_with_localhost_is_not_loopback() {
        for origin in [
            "http://localhost.evil.example",
            "http://notlocalhost",
            "http://localhosts",
        ] {
            assert!(
                !is_loopback_origin(origin),
                "{origin} was treated as loopback"
            );
        }
        assert!(is_loopback_origin("http://app.localhost:8710"));
    }

    /// An RP id must be a domain. Refusing here explains it; passing it on does not.
    #[test]
    fn an_address_origin_is_refused_with_the_reason() {
        assert_eq!(
            relying_party(&proxy_with(Some("http://192.168.1.5:8710")), None)
                .expect_err("an address is not a relying party id"),
            RelyingPartyError::OriginIsAnAddress
        );
        assert_eq!(
            relying_party(&proxy_with(None), Some("http://127.0.0.1:8710"))
                .expect_err("a loopback address is still an address"),
            RelyingPartyError::OriginUnknown
        );
    }

    fn address(last: u8) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, last))
    }

    #[test]
    fn a_ceremony_can_be_claimed_once_and_only_once() {
        let store = CeremonyStore::new();
        let id = store.insert(address(1), "state");
        assert_eq!(store.take(&id), Some("state"));
        assert_eq!(store.take(&id), None);
    }

    #[test]
    fn an_unknown_handle_claims_nothing() {
        let store = CeremonyStore::<&str>::new();
        assert_eq!(store.take("0123456789abcdef0123456789abcdef"), None);
    }

    /// The authentication half is reachable without a session, so the store has to be bounded.
    #[test]
    fn the_store_stays_bounded_under_anonymous_starts() {
        let store = CeremonyStore::new();
        let handles: Vec<_> = (0..MAX_IN_FLIGHT * 4)
            .map(|n| store.insert(address(u8::try_from(n % 200).unwrap_or(0)), n))
            .collect();
        assert!(store.lock().len() <= MAX_IN_FLIGHT);
        // The most recent starts are the ones that survive, so the person actually signing in
        // right now is never the one evicted.
        assert!(
            store
                .take(handles.last().expect("handles were inserted"))
                .is_some()
        );
    }

    /// The eviction that bounds the store must not become the denial of service it prevents.
    ///
    /// The ceiling used to be global and dropped the oldest entry, so an anonymous caller
    /// looping on the challenge endpoint pushed out the challenge the owner was in the middle
    /// of answering. One address flooding must now only cost that address its own entries.
    #[test]
    fn a_flood_from_one_address_does_not_evict_another_address_challenge() {
        let store = CeremonyStore::new();
        let mine = store.insert(address(1), 0_usize);
        for attempt in 1..MAX_IN_FLIGHT * 4 {
            store.insert(address(2), attempt);
        }
        assert!(store.lock().len() <= MAX_IN_FLIGHT);
        assert!(
            store.take(&mine).is_some(),
            "a flood from one address evicted another address's in-flight challenge"
        );
    }

    /// One address cannot hold the whole store either, however slowly it works.
    #[test]
    fn one_address_is_held_to_its_own_share() {
        let store = CeremonyStore::new();
        for attempt in 0..MAX_IN_FLIGHT * 4 {
            store.insert(address(7), attempt);
        }
        assert!(store.lock().len() <= MAX_PER_OWNER);
    }

    #[test]
    fn handles_do_not_repeat() {
        let store = CeremonyStore::new();
        let first = store.insert(address(1), ());
        let second = store.insert(address(1), ());
        assert_ne!(first, second);
        assert_eq!(first.len(), 32, "a handle is 128 bits of hex");
    }
}

//! Who the request actually came from, when there may be a proxy in between.
//!
//! This is the input to rate limiting and to the session inventory, which makes getting it
//! wrong expensive in both directions. Believe a forwarded header from anyone and the client
//! address becomes a string the client chooses — rate limiting is then bypassed by varying it,
//! and the session list becomes fiction. Ignore forwarded headers entirely and every request
//! behind a reverse proxy looks like it came from the proxy, so one blocked attacker locks out
//! everybody.
//!
//! The rule is the standard one and the only defensible one: **a forwarded header is read only
//! when the peer is a proxy the operator named.** Trust is configured, never inferred.
//!
//! Within `X-Forwarded-For`, the list is walked from the right. Each hop appended the address
//! it saw, so the rightmost entries are the ones added by infrastructure closest to us; the
//! first entry from the right that is *not* a trusted proxy is the earliest address that no
//! untrusted party could have chosen. Anything further left was written by whoever spoke to
//! the outermost proxy and is worth nothing.

use std::net::IpAddr;

use crate::cidr::{Cidr, unmap};

/// Header a reverse proxy conventionally uses. Not a standard, but universal.
pub const X_FORWARDED_FOR: &str = "x-forwarded-for";
/// RFC 7239's replacement for it.
pub const FORWARDED: &str = "forwarded";

/// How much of a forwarded chain will be looked at.
///
/// A header with ten thousand entries is not a deployment, it is an attempt to make this loop
/// expensive. The limit is far above any real chain.
const MAX_HOPS: usize = 32;

/// Where the request is treated as coming from, and whether a proxy said so.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientAddress {
    /// The address rate limiting and the session inventory use.
    pub address: IpAddr,
    /// Whether a trusted proxy's forwarded header was believed for this.
    ///
    /// Carried so a diagnostic can say *why* an address was chosen. An operator debugging a
    /// lockout needs to tell "everyone appears to be the proxy" from "the header was read".
    pub from_forwarded_header: bool,
}

/// Decides the client address for one request.
///
/// `peer` is the socket's own remote address — the only thing that cannot be forged.
/// `forwarded_for` and `forwarded` are the raw header values, if present.
#[must_use]
pub fn resolve(
    peer: IpAddr,
    forwarded_for: Option<&str>,
    forwarded: Option<&str>,
    trusted: &[Cidr],
) -> ClientAddress {
    let peer = unmap(peer);
    if !is_trusted(peer, trusted) {
        // The peer is the client. Whatever it claims about earlier hops is its own invention.
        return ClientAddress {
            address: peer,
            from_forwarded_header: false,
        };
    }
    let candidates: Vec<IpAddr> = forwarded_for
        .map(parse_forwarded_for)
        .filter(|hops| !hops.is_empty())
        .or_else(|| {
            forwarded
                .map(parse_forwarded)
                .filter(|hops| !hops.is_empty())
        })
        .unwrap_or_default();

    // Right to left: the first hop that is not itself a trusted proxy.
    for address in candidates.into_iter().rev() {
        if !is_trusted(address, trusted) {
            return ClientAddress {
                address,
                from_forwarded_header: true,
            };
        }
    }
    // A trusted peer that forwarded nothing, or a chain of nothing but trusted proxies. The
    // proxy itself is then genuinely the client — a health check from the sidecar, say.
    ClientAddress {
        address: peer,
        from_forwarded_header: false,
    }
}

/// Whether `address` is one of the operator's proxies.
#[must_use]
pub fn is_trusted(address: IpAddr, trusted: &[Cidr]) -> bool {
    let address = unmap(address);
    trusted.iter().any(|range| range.contains(address))
}

/// `X-Forwarded-For: a, b, c` in order, keeping at most the **rightmost** `MAX_HOPS` entries.
///
/// The side the cap cuts from is the whole point. The rightmost entries are the ones our own
/// infrastructure appended; everything to the left was written by whoever spoke to the
/// outermost proxy. Truncating from the left therefore throws away exactly the trustworthy
/// part and keeps the forged part, so a client that sends `MAX_HOPS` invented entries pushes
/// the proxy's own record of it off the end and gets to name its own address.
fn parse_forwarded_for(value: &str) -> Vec<IpAddr> {
    let mut hops: Vec<IpAddr> = value
        .rsplit(',')
        .take(MAX_HOPS)
        .filter_map(|entry| parse_address(entry.trim()))
        .collect();
    hops.reverse();
    hops
}

/// RFC 7239 `Forwarded: for=a;proto=https, for="[2001:db8::1]:8080"`.
fn parse_forwarded(value: &str) -> Vec<IpAddr> {
    let mut hops: Vec<IpAddr> = value
        .rsplit(',')
        .take(MAX_HOPS)
        .filter_map(|element| {
            element
                .split(';')
                .filter_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    key.trim().eq_ignore_ascii_case("for").then_some(value)
                })
                .find_map(|value| parse_address(value.trim().trim_matches('"')))
        })
        .collect();
    hops.reverse();
    hops
}

/// Reads one entry, with or without a port, bracketed or not.
///
/// Ports appear in both header formats and carry no information here. `unknown` and the
/// obfuscated `_hidden` identifiers RFC 7239 allows simply do not parse, which is the right
/// outcome: an entry that does not name an address cannot be used as one.
fn parse_address(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix('[') {
        let (inside, _) = rest.split_once(']')?;
        return inside.parse().ok().map(unmap);
    }
    if let Ok(address) = value.parse::<IpAddr>() {
        return Some(unmap(address));
    }
    // `1.2.3.4:5678`. Only for v4: a bare v6 with a port is unparseable without brackets, and
    // guessing at it would turn `::1` into something else entirely.
    let (host, port) = value.rsplit_once(':')?;
    if port.parse::<u16>().is_err() {
        return None;
    }
    host.parse::<std::net::Ipv4Addr>().ok().map(IpAddr::V4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("address")
    }

    fn proxies() -> Vec<Cidr> {
        vec![Cidr::parse("10.0.0.0/8").expect("range")]
    }

    /// The case the whole module exists to prevent.
    #[test]
    fn a_forwarded_header_from_an_untrusted_peer_is_ignored() {
        let resolved = resolve(ip("203.0.113.9"), Some("1.2.3.4"), None, &proxies());
        assert_eq!(resolved.address, ip("203.0.113.9"));
        assert!(!resolved.from_forwarded_header);
    }

    /// …and it stays ignored however plausible the claim is.
    #[test]
    fn an_untrusted_peer_cannot_claim_to_be_a_proxy_for_someone_else() {
        for claim in ["10.0.0.1", "127.0.0.1", "::1", "unknown, 10.0.0.1"] {
            let resolved = resolve(ip("198.51.100.5"), Some(claim), None, &proxies());
            assert_eq!(resolved.address, ip("198.51.100.5"), "claimed {claim}");
        }
    }

    #[test]
    fn a_trusted_proxy_speaks_for_its_client() {
        let resolved = resolve(ip("10.0.0.1"), Some("203.0.113.9"), None, &proxies());
        assert_eq!(resolved.address, ip("203.0.113.9"));
        assert!(resolved.from_forwarded_header);
    }

    /// Two proxies in front: the client is the last entry that is not one of ours.
    #[test]
    fn a_chain_is_walked_from_the_right_past_our_own_proxies() {
        let resolved = resolve(
            ip("10.0.0.1"),
            Some("203.0.113.9, 10.0.0.9, 10.0.0.1"),
            None,
            &proxies(),
        );
        assert_eq!(resolved.address, ip("203.0.113.9"));
    }

    /// The attack the right-to-left walk defeats: the client prepends whatever it likes.
    #[test]
    fn entries_a_client_prepended_are_not_believed() {
        let resolved = resolve(
            ip("10.0.0.1"),
            Some("1.1.1.1, 2.2.2.2, 203.0.113.9"),
            None,
            &proxies(),
        );
        assert_eq!(
            resolved.address,
            ip("203.0.113.9"),
            "the rightmost untrusted hop is the earliest address nobody untrusted could choose"
        );
    }

    #[test]
    fn a_trusted_peer_with_no_header_is_itself_the_client() {
        let resolved = resolve(ip("10.0.0.1"), None, None, &proxies());
        assert_eq!(resolved.address, ip("10.0.0.1"));
        assert!(!resolved.from_forwarded_header);
    }

    /// A chain of nothing but our own proxies means the request started at one of them.
    #[test]
    fn a_chain_of_only_trusted_proxies_falls_back_to_the_peer() {
        let resolved = resolve(ip("10.0.0.1"), Some("10.0.0.9, 10.0.0.8"), None, &proxies());
        assert_eq!(resolved.address, ip("10.0.0.1"));
        assert!(!resolved.from_forwarded_header);
    }

    #[test]
    fn the_rfc_7239_header_is_understood_too() {
        let resolved = resolve(
            ip("10.0.0.1"),
            None,
            Some(r#"for=203.0.113.9;proto=https, for="[2001:db8::1]:8080""#),
            &proxies(),
        );
        // The rightmost entry wins, and its brackets and port are stripped.
        assert_eq!(resolved.address, ip("2001:db8::1"));
    }

    /// `X-Forwarded-For` wins when both are present: it is what proxies actually send, and
    /// preferring the one that is present but empty would lose the information in the other.
    #[test]
    fn the_conventional_header_is_preferred_when_both_are_present() {
        let resolved = resolve(
            ip("10.0.0.1"),
            Some("203.0.113.9"),
            Some("for=198.51.100.5"),
            &proxies(),
        );
        assert_eq!(resolved.address, ip("203.0.113.9"));
    }

    /// An empty or unparseable conventional header falls through rather than winning emptily.
    #[test]
    fn an_unusable_conventional_header_falls_through_to_the_rfc_one() {
        let resolved = resolve(
            ip("10.0.0.1"),
            Some("   "),
            Some("for=198.51.100.5"),
            &proxies(),
        );
        assert_eq!(resolved.address, ip("198.51.100.5"));
    }

    #[test]
    fn ports_and_brackets_are_stripped() {
        let resolved = resolve(ip("10.0.0.1"), Some("203.0.113.9:44321"), None, &proxies());
        assert_eq!(resolved.address, ip("203.0.113.9"));
    }

    /// RFC 7239 allows `unknown` and obfuscated identifiers; neither names an address.
    #[test]
    fn entries_that_name_no_address_are_skipped() {
        let resolved = resolve(
            ip("10.0.0.1"),
            Some("203.0.113.9, unknown, _hidden"),
            None,
            &proxies(),
        );
        assert_eq!(resolved.address, ip("203.0.113.9"));
    }

    /// A header long enough to be a denial-of-service attempt is truncated, not walked.
    #[test]
    fn an_absurdly_long_chain_is_bounded() {
        let long = std::iter::repeat_n("10.0.0.1", 5_000)
            .collect::<Vec<_>>()
            .join(", ");
        let resolved = resolve(ip("10.0.0.1"), Some(&long), None, &proxies());
        assert_eq!(resolved.address, ip("10.0.0.1"));
    }

    /// A padded chain must not be able to push the proxy's own entry off the end.
    ///
    /// The client sends far more than `MAX_HOPS` invented entries; the proxy appends the
    /// address it actually saw. Cutting the chain from the left kept only the invented part
    /// and let the client name itself, which is a rate-limit bypass.
    #[test]
    fn padding_the_chain_cannot_displace_the_real_entry() {
        let mut chain = std::iter::repeat_n("198.51.100.7", 200).collect::<Vec<_>>();
        chain.push("203.0.113.9");
        let resolved = resolve(ip("10.0.0.1"), Some(&chain.join(", ")), None, &proxies());
        assert_eq!(resolved.address, ip("203.0.113.9"));
        assert!(resolved.from_forwarded_header);
    }

    /// With nothing configured, nothing is trusted — including loopback.
    ///
    /// The default has to be the safe one. Trusting loopback implicitly would mean a request
    /// that reached us over the loopback interface could name any client it liked.
    #[test]
    fn no_configured_proxies_means_no_header_is_read() {
        let resolved = resolve(ip("127.0.0.1"), Some("203.0.113.9"), None, &[]);
        assert_eq!(resolved.address, ip("127.0.0.1"));
        assert!(!resolved.from_forwarded_header);
    }
}

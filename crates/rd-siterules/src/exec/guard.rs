//! The two bolts on an outgoing address: which hosts a rule may reach, and which addresses
//! nobody may reach.
//!
//! The plugin host has the first one (`validate_request_domain`, re-checked after every
//! redirect by `validate_redirect`); it does not have the second, because a plugin carries a
//! fixed domain list in its signed manifest. A rule's address comes out of the rule, so the
//! ban on private ranges is built here — and it is checked against the *resolved* address,
//! never against the name. A name is free to point wherever its owner likes, and
//! `localtest.me` pointing at `127.0.0.1` is a public name.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

use crate::{format::Rule, text::host_matches};

/// Whether `url` is one the rule may reach: a host its `match` names, or the host of the
/// address the run was given. The second is what the job means by "plus the crawled address
/// itself": a rule revived onto its canonical host must still be able to reach the address
/// the person pasted.
#[must_use]
pub(crate) fn host_allowed(rule: &Rule, origin_host: &str, url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host == origin_host
        || rule
            .matches
            .hosts
            .iter()
            .any(|pattern| host_matches(pattern, host))
}

/// The literal address a URL's host *is*, when it is one rather than a name.
#[must_use]
pub(crate) fn literal_address(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        Host::Ipv4(address) => Some(IpAddr::V4(address)),
        Host::Ipv6(address) => Some(IpAddr::V6(address)),
        Host::Domain(_) => None,
    }
}

/// Whether an address is one a rule may be pointed at: routable, on the public internet, and
/// not this machine or its neighbours.
///
/// Refusing is the default. Anything reserved, local, private or otherwise not a public
/// unicast address is out, and an IPv4 address wearing an IPv6 costume is judged as the IPv4
/// address it is *as well as* by the IPv6 rules: both have to pass.
#[must_use]
pub(crate) fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match embedded_v4(v6) {
            Some(v4) => is_public_v4(v4) && is_public_v6(v6),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast()
        // 0.0.0.0/8, "this network".
        || a == 0
        // 100.64.0.0/10, carrier-grade NAT: the provider's own network, not the internet.
        || (a == 100 && (64..128).contains(&b))
        // 192.0.0.0/24, IETF protocol assignments.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15, benchmarking.
        || (a == 198 && (b == 18 || b == 19))
        // 240.0.0.0/4, reserved.
        || a >= 240)
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        // fc00::/7, unique local.
        || (segments[0] & 0xfe00) == 0xfc00
        // fe80::/10, link local.
        || (segments[0] & 0xffc0) == 0xfe80
        // 100::/64, discard-only (RFC 6666).
        || (segments[0] == 0x0100 && segments[1..4].iter().all(|segment| *segment == 0))
        // 64:ff9b:1::/48, NAT64 with a network-specific prefix (RFC 8215). The embedded
        // address cannot be read out, because RFC 6052 spreads it differently for each of
        // six prefix lengths and the address does not say which one is in force. Refused as
        // a block instead of guessed.
        || (segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001)
        // 2001::/32, Teredo (RFC 4380). The client's IPv4 address is in there, obfuscated
        // by a bitwise NOT, but a Teredo address routes to a relay rather than to that
        // host, so decoding it would judge the wrong machine. Refused as a block.
        || (segments[0] == 0x2001 && segments[1] == 0x0000)
        // 2001:2::/48, benchmarking (RFC 5180).
        || (segments[0] == 0x2001 && segments[1] == 0x0002 && segments[2] == 0)
        // 2001:10::/28 and 2001:20::/28, ORCHID and ORCHIDv2 (RFC 4843, RFC 7343).
        || (segments[0] == 0x2001 && matches!(segments[1] & 0xfff0, 0x0010 | 0x0020))
        // 2001:db8::/32 and 3fff::/20, documentation (RFC 3849, RFC 9637).
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] & 0xfff0) == 0x3ff0)
}

/// The IPv4 address an IPv6 address carries inside it, for the forms that put one there in a
/// place the address itself identifies.
///
/// Without this, `2002:0a00:0001::` — which is `10.0.0.1` in 6to4 clothing — reads as an
/// ordinary global address and passes, both as a request target and as a link handed to the
/// download engine.
///
/// **Deliberately not decoded**, and refused as whole blocks in [`is_public_v6`] instead:
/// Teredo (`2001::/32`), because the embedded address is the client's and the packet goes to
/// a relay; and NAT64 with a network-specific prefix (`64:ff9b:1::/48`), because RFC 6052
/// lays the address out differently for each of six prefix lengths and nothing in the
/// address says which. Anything that carries an IPv4 address without saying so — a tunnel
/// endpoint written into an arbitrary interface identifier — cannot be recognised at all;
/// the host bolt is what bounds that case, since a rule may only point at the hosts its
/// `match` names.
fn embedded_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    let octets = address.octets();
    let last_32 = || Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
    // ::ffff:a.b.c.d -- IPv4-mapped (RFC 4291).
    if let Some(mapped) = address.to_ipv4_mapped() {
        return Some(mapped);
    }
    // 2002:aabb:ccdd::/48 -- 6to4 (RFC 3056): the IPv4 address sits in bits 16 to 47.
    if segments[0] == 0x2002 {
        return Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]));
    }
    // 64:ff9b::/96 -- the NAT64 well-known prefix (RFC 6052): the IPv4 address is the last
    // 32 bits. Only this prefix length is defined for the well-known prefix.
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0)
    {
        return Some(last_32());
    }
    // ...:0000:5efe:a.b.c.d and ...:0200:5efe:a.b.c.d -- an ISATAP interface identifier
    // (RFC 5214), which may sit under any prefix, global ones included. The tunnel endpoint
    // it names is usually inside a site rather than on the internet.
    if matches!(segments[4], 0x0000 | 0x0200) && segments[5] == 0x5efe {
        return Some(last_32());
    }
    // ::a.b.c.d -- IPv4-compatible (deprecated by RFC 4291, still routed by some stacks).
    // `::` and `::1` are not compatible addresses; the IPv6 rules already refuse them.
    if segments[..6].iter().all(|segment| *segment == 0) && !(segments[6] == 0 && segments[7] <= 1)
    {
        return Some(last_32());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::tests::example;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn a_rule_reaches_its_own_hosts_and_the_address_it_was_given() {
        let rule = example();
        assert!(host_allowed(
            &rule,
            "scnlog.me",
            &url("https://scnlog.me/a")
        ));
        assert!(host_allowed(
            &rule,
            "scnlog.me",
            &url("https://cdn.scnlog.me/a")
        ));
        // The address the run was given, even after a revive onto the canonical host.
        assert!(host_allowed(
            &rule,
            "scnlog.eu",
            &url("https://scnlog.eu/a")
        ));
        assert!(!host_allowed(
            &rule,
            "scnlog.me",
            &url("https://evil.test/a")
        ));
        assert!(!host_allowed(&rule, "scnlog.me", &url("ftp://scnlog.me/a")));
        assert!(!host_allowed(
            &rule,
            "scnlog.me",
            &url("file:///etc/passwd")
        ));
    }

    #[test]
    fn every_local_and_private_range_is_refused() {
        for text in [
            "127.0.0.1",
            "127.13.13.13",
            "10.0.0.5",
            "172.16.4.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "192.0.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::192.168.0.1",
            // 6to4 (RFC 3056) around a private, a loopback and a link-local address.
            "2002:0a00:0001::",
            "2002:7f00:0001::1",
            "2002:a9fe:a9fe::",
            // NAT64, well-known prefix (RFC 6052) and network-specific prefix (RFC 8215).
            "64:ff9b::10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b:1::1",
            // ISATAP interface identifiers (RFC 5214), both variants.
            "2606:4700:1:2:0:5efe:10.0.0.1",
            "2600:1f18:1:2:200:5efe:192.168.0.1",
            // Blocks refused whole rather than decoded.
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",
            "2001:10::1",
            "2001:20::1",
            "2001:db8::1",
            "3fff::1",
            "100::1",
        ] {
            let address: IpAddr = text.parse().expect("address");
            assert!(!is_public(address), "{text} was allowed");
        }
    }

    #[test]
    fn a_routable_address_is_allowed() {
        for text in [
            "93.184.216.34",
            "8.8.8.8",
            "172.32.0.1",
            "2606:4700::1111",
            // 6to4 and NAT64 around routable IPv4 addresses stay routable.
            "2002:5db8:d822::1",
            "64:ff9b::93.184.216.34",
        ] {
            let address: IpAddr = text.parse().expect("address");
            assert!(is_public(address), "{text} was refused");
        }
    }

    #[test]
    fn a_literal_address_is_recognised_in_the_host() {
        assert_eq!(
            literal_address(&url("http://127.0.0.1:8710/x")),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(
            literal_address(&url("http://[::1]/x")),
            Some(IpAddr::V6(Ipv6Addr::LOCALHOST))
        );
        assert_eq!(literal_address(&url("https://scnlog.me/x")), None);
    }
}

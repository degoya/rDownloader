//! How far from the internet an IP address is: the one classification every outgoing-request
//! guard of the service uses.
//!
//! `rd-http` (`address_guard`, RD-150-03) and `rd-siterules` (`exec/guard.rs`) each kept their
//! own copy of these ranges, and the copies drifted: the site-rule guard let the deprecated
//! site-local block `fec0::/10` through while the download engine refused it (security audit
//! 2026-09-30, R1). Here rather than in `rd-http` because `rd-pack` builds `rd-siterules`
//! without the HTTP engine; both guards now ask this one function.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// How far from the internet an address is. Ordered from the most to the least reachable, so
/// the stricter of two judgements is their maximum.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AddressScope {
    /// Routable on the internet.
    Public,
    /// The person's own network: RFC 1918, carrier-grade NAT, IPv6 unique-local and the
    /// deprecated site-local block.
    Private,
    /// Never the home of a remote file: loopback, unspecified, link-local (the cloud metadata
    /// endpoint among it), multicast, broadcast and the reserved and special-purpose blocks.
    Local,
}

/// Which scope an address belongs to. An IPv4 address wearing an IPv6 costume is judged as the
/// IPv4 address it carries *as well as* by the IPv6 rules, and the stricter answer wins.
#[must_use]
pub fn address_scope(address: IpAddr) -> AddressScope {
    match address {
        IpAddr::V4(v4) => scope_v4(v4),
        IpAddr::V6(v6) => {
            let own = scope_v6(v6);
            embedded_v4(v6).map_or(own, |v4| own.max(scope_v4(v4)))
        }
    }
}

fn scope_v4(address: Ipv4Addr) -> AddressScope {
    let [a, b, c, _] = address.octets();
    let local = address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast()
        // 0.0.0.0/8, "this network".
        || a == 0
        // 192.0.0.0/24, IETF protocol assignments.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15, benchmarking.
        || (a == 198 && (b == 18 || b == 19))
        // 240.0.0.0/4, reserved.
        || a >= 240;
    if local {
        AddressScope::Local
    } else if address.is_private()
        // 100.64.0.0/10, carrier-grade NAT: the provider's network, not the internet.
        || (a == 100 && (64..128).contains(&b))
    {
        AddressScope::Private
    } else {
        AddressScope::Public
    }
}

fn scope_v6(address: Ipv6Addr) -> AddressScope {
    let segments = address.segments();
    let local = address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        // fe80::/10, link local.
        || (segments[0] & 0xffc0) == 0xfe80
        // 100::/64, discard-only (RFC 6666).
        || (segments[0] == 0x0100 && segments[1..4].iter().all(|segment| *segment == 0))
        // 64:ff9b:1::/48, NAT64 with a network-specific prefix (RFC 8215): the embedded
        // address cannot be read out without knowing the prefix length, so the block goes.
        || (segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001)
        // 2001::/32, Teredo: the packet goes to a relay, not to the address inside.
        || (segments[0] == 0x2001 && segments[1] == 0x0000)
        // 2001:2::/48, benchmarking (RFC 5180).
        || (segments[0] == 0x2001 && segments[1] == 0x0002 && segments[2] == 0)
        // 2001:10::/28 and 2001:20::/28, ORCHID and ORCHIDv2.
        || (segments[0] == 0x2001 && matches!(segments[1] & 0xfff0, 0x0010 | 0x0020))
        // 2001:db8::/32 and 3fff::/20, documentation.
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] & 0xfff0) == 0x3ff0;
    if local {
        AddressScope::Local
    } else if (segments[0] & 0xfe00) == 0xfc00 || (segments[0] & 0xffc0) == 0xfec0 {
        // fc00::/7 unique local, fec0::/10 the site-local block it replaced.
        AddressScope::Private
    } else {
        AddressScope::Public
    }
}

/// The IPv4 address an IPv6 address carries where the address itself says so: IPv4-mapped,
/// 6to4, the NAT64 well-known prefix, an ISATAP interface identifier and IPv4-compatible.
/// Without this, `::ffff:127.0.0.1` reads as an ordinary IPv6 address.
///
/// **Deliberately not decoded**, and refused as whole blocks in `scope_v6` instead: Teredo
/// (`2001::/32`), because the embedded address is the client's and the packet goes to a relay;
/// and NAT64 with a network-specific prefix (`64:ff9b:1::/48`), because RFC 6052 lays the
/// address out differently for each of six prefix lengths and nothing in the address says which.
fn embedded_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    let octets = address.octets();
    let last_32 = || Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
    if let Some(mapped) = address.to_ipv4_mapped() {
        return Some(mapped);
    }
    // 2002:aabb:ccdd::/48, 6to4: the IPv4 address sits in bits 16 to 47.
    if segments[0] == 0x2002 {
        return Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]));
    }
    // 64:ff9b::/96, the NAT64 well-known prefix: the last 32 bits.
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0)
    {
        return Some(last_32());
    }
    // ...:0000:5efe:a.b.c.d and ...:0200:5efe:a.b.c.d, ISATAP under any prefix.
    if matches!(segments[4], 0x0000 | 0x0200) && segments[5] == 0x5efe {
        return Some(last_32());
    }
    // ::a.b.c.d, IPv4-compatible; `::` and `::1` are not, and the IPv6 rules refuse them.
    if segments[..6].iter().all(|segment| *segment == 0) && !(segments[6] == 0 && segments[7] <= 1)
    {
        return Some(last_32());
    }
    None
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{AddressScope, address_scope};

    fn scope(text: &str) -> AddressScope {
        address_scope(text.parse::<IpAddr>().expect("address"))
    }

    /// The union of what the two guards used to refuse, each range in one place now.
    ///
    /// `fec0::1` is the address the copies disagreed on: the download engine refused it as
    /// private, the site-rule guard waved it through as public.
    #[test]
    fn every_range_either_guard_refused_is_refused() {
        for text in [
            "127.0.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "192.0.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "ff02::1",
            "100::1",
            "64:ff9b:1::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",
            "2001:10::1",
            "2001:20::1",
            "2001:db8::1",
            "3fff::1",
            "::ffff:127.0.0.1",
            "2002:7f00:0001::1",
            "64:ff9b::7f00:1",
            "2606:4700:1:2:0:5efe:127.0.0.1",
        ] {
            assert_eq!(scope(text), AddressScope::Local, "{text}");
        }
        for text in [
            "10.0.0.5",
            "172.16.4.1",
            "192.168.1.1",
            "100.64.0.1",
            "fc00::1",
            "fd12:3456::1",
            "fec0::1",
            "::ffff:10.1.2.3",
            "2002:0a00:0001::",
            "64:ff9b::10.0.0.1",
            "2600:1f18:1:2:200:5efe:192.168.0.1",
            "::192.168.0.1",
        ] {
            assert_eq!(scope(text), AddressScope::Private, "{text}");
        }
    }

    #[test]
    fn a_routable_address_is_public() {
        for text in [
            "93.184.216.34",
            "8.8.8.8",
            "172.32.0.1",
            "2606:4700::1111",
            "2002:5db8:d822::1",
            "64:ff9b::93.184.216.34",
        ] {
            assert_eq!(scope(text), AddressScope::Public, "{text}");
        }
    }
}

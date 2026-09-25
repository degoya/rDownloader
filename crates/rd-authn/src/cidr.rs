//! Address ranges, for deciding which hop is allowed to speak for the client.
//!
//! Hand-rolled rather than pulled in as a dependency. What is needed is prefix comparison on
//! four or sixteen bytes; a crate for that would add a supply-chain entry and a `cargo-deny`
//! exception to save forty lines, and the forty lines are the part that has to be right.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One address range, written the way an operator writes it: `10.0.0.0/8`, `::1/128`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

/// Why a range could not be read.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum CidrError {
    /// Not an address, with or without a prefix.
    #[error("`{0}` is not an IP address or CIDR range")]
    Malformed(String),
    /// A prefix longer than the address family allows.
    #[error("`{value}` has a /{prefix} prefix, but the address family has only {bits} bits")]
    PrefixTooLong { value: String, prefix: u8, bits: u8 },
}

impl Cidr {
    /// Parses `addr/prefix`, or a bare address as a single host.
    ///
    /// A bare address is accepted because that is what an operator writes for one proxy, and
    /// rejecting it would only produce `/32` suffixes nobody wants to think about.
    pub fn parse(value: &str) -> Result<Self, CidrError> {
        let value = value.trim();
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (
                address,
                Some(
                    prefix
                        .parse::<u8>()
                        .map_err(|_| CidrError::Malformed(value.to_owned()))?,
                ),
            ),
            None => (value, None),
        };
        let network: IpAddr = address
            .parse()
            .map_err(|_| CidrError::Malformed(value.to_owned()))?;
        let bits = if network.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(bits);
        if prefix > bits {
            return Err(CidrError::PrefixTooLong {
                value: value.to_owned(),
                prefix,
                bits,
            });
        }
        Ok(Self { network, prefix })
    }

    /// Whether `address` falls inside this range.
    ///
    /// An IPv4 address written as an IPv4-mapped IPv6 (`::ffff:10.0.0.1`) is unmapped first.
    /// A dual-stack listener reports exactly that shape, so without the unmapping an operator
    /// who wrote `10.0.0.0/8` would find their proxy untrusted for reasons invisible to them.
    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        let address = unmap(address);
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                prefix_matches(&network.octets(), &address.octets(), self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                prefix_matches(&network.octets(), &address.octets(), self.prefix)
            }
            // Families do not mix: a v4 range never covers a v6 address, mapped ones aside.
            _ => false,
        }
    }
}

/// Turns an IPv4-mapped IPv6 address back into the IPv4 address it stands for.
#[must_use]
pub fn unmap(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    }
}

fn prefix_matches(network: &[u8], address: &[u8], prefix: u8) -> bool {
    let whole = usize::from(prefix / 8);
    let remainder = prefix % 8;
    if network[..whole] != address[..whole] {
        return false;
    }
    if remainder == 0 {
        return true;
    }
    let mask = 0xFF_u8 << (8 - remainder);
    network[whole] & mask == address[whole] & mask
}

/// The loopback ranges, trusted by default because that is where a co-located proxy sits.
#[must_use]
pub fn loopback_ranges() -> Vec<Cidr> {
    vec![
        Cidr {
            network: IpAddr::V4(Ipv4Addr::LOCALHOST),
            prefix: 8,
        },
        Cidr {
            network: IpAddr::V6(Ipv6Addr::LOCALHOST),
            prefix: 128,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("address")
    }

    #[test]
    fn a_range_contains_its_own_network_and_broadcast() {
        let range = Cidr::parse("10.0.0.0/8").expect("range");
        assert!(range.contains(ip("10.0.0.0")));
        assert!(range.contains(ip("10.255.255.255")));
        assert!(range.contains(ip("10.1.2.3")));
        assert!(!range.contains(ip("11.0.0.1")));
        assert!(!range.contains(ip("9.255.255.255")));
    }

    /// The bit-level boundary, which a byte-wise comparison alone would get wrong.
    #[test]
    fn a_prefix_that_is_not_a_whole_number_of_bytes_is_respected() {
        let range = Cidr::parse("192.168.16.0/20").expect("range");
        assert!(range.contains(ip("192.168.16.1")));
        assert!(range.contains(ip("192.168.31.255")));
        assert!(!range.contains(ip("192.168.32.0")));
        assert!(!range.contains(ip("192.168.15.255")));
    }

    #[test]
    fn a_bare_address_is_a_single_host() {
        let range = Cidr::parse("203.0.113.7").expect("range");
        assert!(range.contains(ip("203.0.113.7")));
        assert!(!range.contains(ip("203.0.113.8")));
    }

    /// A dual-stack listener reports IPv4 peers in this shape; treating it as "some v6
    /// address" would silently untrust every proxy an operator configured by v4 range.
    #[test]
    fn an_ipv4_mapped_address_matches_the_ipv4_range() {
        let range = Cidr::parse("10.0.0.0/8").expect("range");
        assert!(range.contains(ip("::ffff:10.1.2.3")));
        assert!(!range.contains(ip("::ffff:11.1.2.3")));
    }

    #[test]
    fn the_families_do_not_mix() {
        assert!(
            !Cidr::parse("10.0.0.0/8")
                .expect("range")
                .contains(ip("::1"))
        );
        assert!(
            !Cidr::parse("fd00::/8")
                .expect("range")
                .contains(ip("10.0.0.1"))
        );
    }

    /// `/0` means everything, and it has to mean that rather than nothing.
    #[test]
    fn a_zero_prefix_covers_the_whole_family() {
        let range = Cidr::parse("0.0.0.0/0").expect("range");
        assert!(range.contains(ip("1.2.3.4")));
        assert!(range.contains(ip("255.255.255.255")));
    }

    #[test]
    fn a_malformed_range_is_refused_rather_than_widened() {
        assert!(matches!(
            Cidr::parse("not-an-address"),
            Err(CidrError::Malformed(_))
        ));
        assert!(matches!(
            Cidr::parse("10.0.0.0/"),
            Err(CidrError::Malformed(_))
        ));
        assert!(matches!(
            Cidr::parse("10.0.0.0/33"),
            Err(CidrError::PrefixTooLong { .. })
        ));
        assert!(matches!(
            Cidr::parse("::1/129"),
            Err(CidrError::PrefixTooLong { .. })
        ));
    }

    #[test]
    fn loopback_is_covered_by_the_default_ranges() {
        let ranges = loopback_ranges();
        assert!(ranges.iter().any(|range| range.contains(ip("127.0.0.1"))));
        assert!(ranges.iter().any(|range| range.contains(ip("127.5.5.5"))));
        assert!(ranges.iter().any(|range| range.contains(ip("::1"))));
        assert!(!ranges.iter().any(|range| range.contains(ip("10.0.0.1"))));
    }
}

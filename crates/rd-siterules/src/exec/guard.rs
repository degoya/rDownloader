//! The two bolts on an outgoing address: which hosts a rule may reach, and which addresses
//! nobody may reach.
//!
//! The plugin host has the first one (`validate_request_domain`, re-checked after every
//! redirect by `validate_redirect`); it does not have the second, because a plugin carries a
//! fixed domain list in its signed manifest. A rule's address comes out of the rule, so the
//! ban on private ranges is applied here — with the ranges of `rd_core::address_scope`, which
//! the download engine's guard uses too — and it is checked against the *resolved* address,
//! never against the name. A name is free to point wherever its owner likes, and
//! `localtest.me` pointing at `127.0.0.1` is a public name.

use std::net::IpAddr;

use url::Url;

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

pub(crate) use rd_core::literal_address;

/// Whether an address is one a rule may be pointed at: routable, on the public internet, and
/// not this machine or its neighbours.
///
/// Refusing is the default: anything but [`rd_core::AddressScope::Public`] is out. The ranges
/// are `rd_core::address_scope`, the classification the download engine's guard uses too; this
/// crate kept its own copy until the security audit of 2026-09-30 (R1) found it had let the
/// site-local block `fec0::/10` through while the engine refused it.
#[must_use]
pub(crate) fn is_public(address: IpAddr) -> bool {
    rd_core::address_scope(address) == rd_core::AddressScope::Public
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

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
            // Site-local (RFC 3879): the range the two copies of this list disagreed on.
            "fec0::1",
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

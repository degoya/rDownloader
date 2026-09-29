//! Which names a request may call this service by.
//!
//! DNS rebinding is the attack this answers: a page on `attacker.example` points its own name
//! at `127.0.0.1` once it has loaded, and from then on the browser treats this service as the
//! page's own origin — same-origin requests, readable answers, no CORS in the way. The browser
//! still sends the name it thinks it is talking to in `Host`, and that name is the one thing
//! the attacker cannot choose: it is their own domain.
//!
//! So a `Host` is accepted when it is
//!
//! - an IP literal — the listener's own addresses among them, and every LAN address somebody
//!   types into a browser. Rebinding needs a DNS name; an address is never one;
//! - `localhost` or a name under `.localhost`, which browsers resolve to loopback themselves
//!   and never ask DNS about (RFC 6761);
//! - a name that exists only on the local network and that nobody can register in public DNS:
//!   a single label (`nas`, `syno-xyz`), a multicast DNS name under `.local` (RFC 6762), or a
//!   name under the reserved home and private suffixes `.home.arpa` (RFC 8375) and `.internal`.
//!   An attacker's page carries the attacker's own public name, which is never one of these;
//! - the host of the configured external URL;
//! - one of the names the operator added to the allowed host list.
//!
//! Every other name is refused, however it resolves.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// What a `Host` value names, with the port taken off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestHost {
    /// An IPv4 or IPv6 literal.
    Address(IpAddr),
    /// A DNS name, lowercased and without a trailing dot.
    Name(String),
}

/// Reads a `Host` header or a URI authority's host: `name`, `name:port`, `1.2.3.4:port`,
/// `[::1]:port`. `None` for anything that is none of those, which is refused.
#[must_use]
pub fn parse_request_host(value: &str) -> Option<RequestHost> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        if !(after.is_empty() || is_port(after.strip_prefix(':')?)) {
            return None;
        }
        return inside
            .parse::<Ipv6Addr>()
            .ok()
            .map(|address| RequestHost::Address(IpAddr::V6(address)));
    }
    let name = match value.split_once(':') {
        Some((name, port)) if is_port(port) => name,
        Some(_) => return None,
        None => value,
    };
    if let Ok(address) = name.parse::<Ipv4Addr>() {
        return Some(RequestHost::Address(IpAddr::V4(address)));
    }
    normalise_name(name).map(RequestHost::Name)
}

/// Reads one entry of the allowed host list: a bare DNS name, no scheme, port or path.
/// `None` when it is not one.
#[must_use]
pub fn parse_allowed_host(value: &str) -> Option<String> {
    let value = value.trim();
    if value.parse::<IpAddr>().is_ok() {
        // An address is accepted anyway; keeping it in the list is harmless and saves a
        // refusal nobody would understand.
        return Some(value.to_owned());
    }
    normalise_name(value)
}

/// Whether a request naming `host` may be served.
///
/// `external` is the external URL's host, `allowed` the operator's list, both already
/// normalised.
#[must_use]
pub fn host_permitted(host: &RequestHost, external: Option<&str>, allowed: &[String]) -> bool {
    let name = match host {
        RequestHost::Address(_) => return true,
        RequestHost::Name(name) => name.as_str(),
    };
    name == "localhost"
        || name.ends_with(".localhost")
        || is_local_only(name)
        || external == Some(name)
        || allowed.iter().any(|entry| entry == name)
}

/// A name public DNS cannot hand out: one label, or under a suffix reserved for local use.
fn is_local_only(name: &str) -> bool {
    !name.contains('.')
        || [".local", ".home.arpa", ".internal"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Lowercases a DNS name and drops a trailing dot; `None` when it is not a name.
fn normalise_name(value: &str) -> Option<String> {
    let name = value
        .strip_suffix('.')
        .unwrap_or(value)
        .to_ascii_lowercase();
    let valid = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        });
    valid.then_some(name)
}

fn is_port(value: &str) -> bool {
    !value.is_empty() && value.len() <= 5 && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permitted(value: &str, external: Option<&str>, allowed: &[&str]) -> bool {
        let allowed: Vec<String> = allowed.iter().map(|entry| (*entry).to_owned()).collect();
        parse_request_host(value).is_some_and(|host| host_permitted(&host, external, &allowed))
    }

    #[test]
    fn every_address_literal_is_permitted() {
        for value in [
            "127.0.0.1",
            "127.0.0.1:8710",
            "192.168.1.20:8710",
            "10.0.0.5",
            "[::1]",
            "[::1]:8710",
            "[fe80::1]:8710",
        ] {
            assert!(permitted(value, None, &[]), "{value}");
        }
    }

    #[test]
    fn localhost_is_permitted_in_any_case_and_with_a_port() {
        for value in [
            "localhost",
            "LOCALHOST:8710",
            "localhost.",
            "app.localhost:8710",
        ] {
            assert!(permitted(value, None, &[]), "{value}");
        }
    }

    /// The rebinding case: a name the attacker owns, whatever it resolves to right now.
    #[test]
    fn an_unknown_name_is_refused() {
        for value in [
            "attacker.example",
            "attacker.example:8710",
            "localhost.attacker.example",
            "nas.lan",
            "local.attacker.example",
            "internal.attacker.example",
            "home.arpa.attacker.example",
            "rd.example.com.evil.test",
        ] {
            assert!(!permitted(value, None, &[]), "{value}");
        }
    }

    /// Names that only exist on the local network: nobody can register them in public DNS, so
    /// no page can rebind to one of them.
    #[test]
    fn a_local_only_name_is_permitted() {
        for value in [
            "nas",
            "syno-xyz:8710",
            "SYNO-XYZ.",
            "syno-xyz.local:8710",
            "nas.home.arpa",
            "rdownloader.internal:8710",
        ] {
            assert!(permitted(value, None, &[]), "{value}");
        }
    }

    #[test]
    fn the_external_host_and_the_listed_names_are_permitted() {
        assert!(permitted("rd.example.com", Some("rd.example.com"), &[]));
        assert!(permitted("RD.example.com:443", Some("rd.example.com"), &[]));
        assert!(permitted("nas.lan:8710", None, &["nas.lan"]));
        assert!(permitted("nas.lan.", None, &["nas.lan"]));
        assert!(!permitted(
            "other.lan",
            Some("rd.example.com"),
            &["nas.lan"]
        ));
    }

    #[test]
    fn a_malformed_host_is_refused() {
        for value in [
            "",
            ":8710",
            "::1",
            "[::1",
            "[::1]x",
            "[not-v6]:80",
            "host:port",
            "host:80:80",
            "a b",
            "user@host",
            "a..b",
        ] {
            assert!(!permitted(value, None, &[]), "{value:?}");
        }
    }

    #[test]
    fn a_list_entry_must_be_a_bare_name() {
        assert_eq!(parse_allowed_host(" NAS.lan. "), Some("nas.lan".to_owned()));
        assert_eq!(
            parse_allowed_host("192.168.1.2"),
            Some("192.168.1.2".to_owned())
        );
        for value in [
            "http://nas.lan",
            "nas.lan:8710",
            "nas.lan/path",
            "*.lan",
            "",
            "a b",
        ] {
            assert_eq!(parse_allowed_host(value), None, "{value:?}");
        }
    }
}

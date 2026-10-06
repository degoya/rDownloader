//! Reading an address the way the plugins that claim one read it, without a URL parser
//! (RD-1120-10, PL-5 and PL-22).
//!
//! Five plugins carried the same `split` and `query_value` and the same identifier checks, byte
//! for byte, and a check reimplemented per provider is a check that drifts: the one that keeps
//! `x@evil.test` from reading as a provider's own host has to be the same everywhere.
//!
//! [`Parts`] is for a plugin that used the `url` crate only to find a host and a path. That
//! crate brings its IDNA tables into every component that links it — a crawler of 150 KiB
//! becomes one of 340 — for a question a host list answers. What it reads differently is named
//! on the type; every difference refuses an address rather than accepting one.
//!
//! Plain Rust with no dependencies, so a guest that takes it gains no import.

/// Host and the rest of an address — path and query, without the fragment.
///
/// Returns `None` for anything that is not plain http(s), and for an authority carrying
/// credentials — accepting those would let `x@evil.test` read as a provider's own host. The
/// port is dropped; the host is returned as written, so a caller compares it without case.
#[must_use]
pub fn split(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let rest = rest.split('#').next()?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return None;
    }
    Some((authority.split(':').next()?, path))
}

/// The value of one `name=value` parameter of a query, undecoded; the first one wins.
#[must_use]
pub fn parameter<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// The value of one query parameter of a path that may carry a query, undecoded.
#[must_use]
pub fn query_value<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    parameter(path.split_once('?')?.1, name)
}

/// An identifier as providers issue them for addresses — ASCII letters, digits, `-` and `_`,
/// one to `max` bytes.
///
/// Bounded and character-checked rather than trusted, because such a value is pasted straight
/// into an API parameter or an address. Anything else could be a path, an escape or a second
/// address.
#[must_use]
pub fn valid_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// One DNS label — a tenant's or an enterprise's own subdomain: letters, digits and `-`, at
/// most 63 bytes, and never a dot, so it cannot smuggle in a host of its own.
#[must_use]
pub fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.contains('.')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// A file or folder name as one segment of a path: non-empty, at most 255 bytes, not a dot
/// entry, no separator and no control character.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|character| matches!(character, '/' | '\\') || character.is_control())
}

/// A digest as an API states it: exactly `length` hexadecimal characters.
#[must_use]
pub fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// An absolute address taken apart into what a claim looks at.
///
/// Read like a browser reads one, as far as a claim needs: spaces and control characters around
/// it are ignored, the fragment is dropped, the authority ends at the first `/` or `?`, the port
/// is dropped (an IPv6 literal keeps its brackets). What it does not do is map: an IDNA or
/// percent-encoded spelling of a host stays as written and so matches no host list, a `\` is
/// not read as a `/` but refuses the address, and a dot segment is not resolved but refuses it
/// ([`Parts::segments`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Parts<'a> {
    /// As written; [`Parts::is_http`] compares it.
    pub scheme: &'a str,
    /// Whether the authority carried a user name or a password, even an empty one.
    pub credentials: bool,
    /// As written, without the port; compare it with ASCII case ignored.
    pub host: &'a str,
    /// From the first `/` up to the query, `""` when there is none; escapes left as they are.
    pub path: &'a str,
    /// What follows the first `?`, up to the fragment.
    pub query: Option<&'a str>,
}

impl<'a> Parts<'a> {
    /// Takes an absolute address apart, or `None` when it is not one.
    #[must_use]
    pub fn of(url: &'a str) -> Option<Self> {
        let url = url.trim_matches(|character: char| character <= ' ');
        let (scheme, rest) = url.split_once("://")?;
        if !valid_scheme(scheme) {
            return None;
        }
        let rest = rest.split('#').next()?;
        let (before_query, query) = match rest.split_once('?') {
            Some((before, query)) => (before, Some(query)),
            None => (rest, None),
        };
        if before_query.contains('\\') {
            return None;
        }
        let (authority, path) = match before_query.find('/') {
            Some(at) => before_query.split_at(at),
            None => (before_query, ""),
        };
        let (credentials, host_and_port) = match authority.rsplit_once('@') {
            Some((_, host_and_port)) => (true, host_and_port),
            None => (false, authority),
        };
        let host = without_port(host_and_port)?;
        if host.is_empty() {
            return None;
        }
        Some(Self {
            scheme,
            credentials,
            host,
            path,
            query,
        })
    }

    /// Whether the scheme is `http` or `https`, in any case.
    #[must_use]
    pub fn is_http(&self) -> bool {
        self.scheme.eq_ignore_ascii_case("http") || self.scheme.eq_ignore_ascii_case("https")
    }

    /// The path's segments without the empty ones, or `None` when one of them is a dot segment
    /// (`.`, `..`, or either spelled with `%2e`): a browser resolves those against their
    /// neighbours, and reading them as names would claim an address the browser reads as
    /// another one.
    #[must_use]
    pub fn segments(&self) -> Option<Vec<&'a str>> {
        let segments: Vec<&str> = self
            .path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        (!segments.iter().any(|segment| is_dot_segment(segment))).then_some(segments)
    }
}

/// RFC 3986's scheme: a letter, then letters, digits, `+`, `-` and `.`.
fn valid_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

/// The host of `host[:port]`, or `None` when the port is not a port.
fn without_port(authority: &str) -> Option<&str> {
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        let (host, after) = authority.split_at(close + 1);
        if after.is_empty() {
            (host, None)
        } else {
            (host, Some(after.strip_prefix(':')?))
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    match port {
        Some(port) if !port.is_empty() && port.parse::<u16>().is_err() => None,
        Some(port) if !port.bytes().all(|byte| byte.is_ascii_digit()) => None,
        _ => Some(host),
    }
}

fn is_dot_segment(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "." | ".." | "%2e" | ".%2e" | "%2e." | "%2e%2e"
    )
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;

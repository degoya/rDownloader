//! What this plugin claims, and the address arithmetic every other module needs.
//!
//! Deliberately not the `url` crate: a plugin component links nothing outside
//! `rdownloader:plugin`, and the three questions this file answers — is it a web address,
//! does its path end in a slash, is this link strictly below that path — are smaller than a
//! parser. Everything here is a decision about text, and every decision is tested.

/// A web address split into the three parts this plugin reasons about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Address {
    /// `http` or `https`, lowercase.
    pub scheme: String,
    /// Host and optional port, lowercase.
    pub authority: String,
    /// Always starts with `/`; the query and the fragment are not part of it.
    pub path: String,
}

impl Address {
    /// The address as one string again, without query or fragment.
    #[must_use]
    pub fn to_url(&self) -> String {
        format!("{}://{}{}", self.scheme, self.authority, self.path)
    }

    /// The origin this address belongs to.
    #[must_use]
    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme, self.authority)
    }
}

/// Splits a web address. `None` for anything that is not `http`/`https` with a host.
#[must_use]
pub fn parse(url: &str) -> Option<Address> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.is_empty() || authority.contains('@') {
        // Credentials in the address are not something a crawl carries around; a listing
        // that needs them is not an open listing.
        return None;
    }
    let path = tail.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    Some(Address {
        scheme,
        authority: authority.to_ascii_lowercase(),
        path: path.to_owned(),
    })
}

/// Whether this plugin claims `url`, answered from the address alone and reaching nothing.
///
/// The shape is the whole test, and it is the narrowest shape that still covers the four
/// servers: a path that ends in a slash. An address with a query string is a page that does
/// something, not a directory; one ending in a file name is a file somebody else resolves.
/// Being occasionally wrong is expected and survivable — `crawl` says `unsupported` and the
/// selection carries the address on (RD-107-05).
#[must_use]
pub fn claim(url: &str) -> Option<Address> {
    if url.contains('?') {
        return None;
    }
    let address = parse(url)?;
    address.path.ends_with('/').then_some(address)
}

/// The name of the deepest directory in a path, percent-decoded; empty for the site root.
#[must_use]
pub fn directory_name(path: &str) -> String {
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    decode(name)
}

/// Percent-decodes one path segment, leaving anything malformed as it was.
///
/// Lossy on purpose: the result is a file name to show, not bytes to send anywhere, and a
/// server that emitted an invalid sequence should not be able to end the crawl over it.
#[must_use]
pub fn decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{claim, decode, directory_name, parse};

    #[test]
    fn a_path_that_ends_in_a_slash_is_claimed_and_nothing_else_is() {
        assert!(claim("https://files.example.org/pub/").is_some());
        assert!(claim("http://files.example.org/").is_some());
        // A file, a page with a query, and anything that is not a web address.
        assert!(claim("https://files.example.org/pub/disc.iso").is_none());
        assert!(claim("https://files.example.org/pub/?C=M;O=D").is_none());
        assert!(claim("ftp://files.example.org/pub/").is_none());
        assert!(claim("magnet:?xt=urn:btih:abc").is_none());
        // Credentials in the address: not an open listing.
        assert!(claim("https://me:pw@files.example.org/pub/").is_none());
    }

    #[test]
    fn an_address_splits_into_scheme_authority_and_path() {
        let address = parse("HTTPS://Files.Example.ORG:8443/pub/x/?a=b#c").expect("address");
        assert_eq!(address.scheme, "https");
        assert_eq!(address.authority, "files.example.org:8443");
        assert_eq!(address.path, "/pub/x/");
        assert_eq!(address.origin(), "https://files.example.org:8443");
        assert_eq!(address.to_url(), "https://files.example.org:8443/pub/x/");
        // A bare host is the site root.
        assert_eq!(parse("https://example.org").expect("root").path, "/");
    }

    #[test]
    fn a_directory_name_is_the_deepest_segment_decoded() {
        assert_eq!(directory_name("/pub/Ubuntu%2024.04/"), "Ubuntu 24.04");
        assert_eq!(directory_name("/pub/"), "pub");
        assert_eq!(directory_name("/"), "");
    }

    #[test]
    fn a_malformed_escape_survives_instead_of_ending_the_crawl() {
        assert_eq!(decode("a%20b"), "a b");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("%C3%A4"), "\u{e4}");
    }
}

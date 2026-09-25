//! What this plugin claims, and the address arithmetic the rest of it needs.
//!
//! Deliberately not the `url` crate: a component links nothing outside `rdownloader:plugin`,
//! and the questions here — is this one of the two domains, is the path a single entry
//! identifier, did the service redirect us off that entry — are smaller than a parser.

/// The domains the service answers on, measured on 2026-09-21.
///
/// `alfalink.info`, which `alfalink.to/robots.txt` names as its `Host` and which the
/// JDownloader pattern still carries, is **not** here: it ran into a 25 s timeout. It is kept
/// in [`SERVICE_HOSTS`] so a link back to it is still recognised as the service's own and
/// dropped, but an address on it is not claimed — claiming one would promise a request the
/// manifest does not grant.
const CLAIMED_HOSTS: [&str; 2] = ["peeplink.in", "alfalink.to"];

/// Every host that is the service itself, including the dead alias.
///
/// Used to drop the service's own links out of an entry page, so a "report this entry" or a
/// front-page link never becomes a download.
const SERVICE_HOSTS: [&str; 3] = ["peeplink.in", "alfalink.to", "alfalink.info"];

/// Shortest and longest entry identifier accepted.
///
/// The four measured identifiers are 12 hexadecimal characters on `peeplink.in` and 22 on
/// `alfalink.to`. The range is wider than both so a service that lengthens its identifiers
/// does not need a release, and narrow enough that `/tos.html`, `/index.php` and the rest of
/// the site are not claimed.
const ID_LENGTH: std::ops::RangeInclusive<usize> = 8..=40;

/// One entry address, split into the parts this plugin reasons about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// `http` or `https`, lowercase.
    pub scheme: String,
    /// Host without a port, lowercase, with any `www.` kept as pasted.
    pub authority: String,
    /// The entry identifier, lowercase.
    pub id: String,
    /// The access password, when the address carried one after `#`.
    pub password: Option<String>,
}

impl Entry {
    /// The address to request. The fragment is not part of it: a fragment is never sent.
    #[must_use]
    pub fn to_url(&self) -> String {
        format!("{}://{}/{}", self.scheme, self.authority, self.id)
    }
}

/// Whether this plugin claims `url`, answered from the address alone and reaching nothing.
///
/// Narrow on purpose: two named hosts and a path that is one hexadecimal identifier. Unlike a
/// crawler that claims by shape, this one cannot be wrong about somebody else's site.
#[must_use]
pub fn claim(url: &str) -> Option<Entry> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let authority = authority.to_ascii_lowercase();
    if !is_claimed_host(&authority) {
        return None;
    }
    // The password rides after `#`, which never reaches a server: a fragment is not sent.
    let (before_fragment, fragment) = match tail.split_once('#') {
        Some((before, after)) => (before, Some(after)),
        None => (tail, None),
    };
    let path = before_fragment.split('?').next().unwrap_or("");
    let id = entry_id(path)?;
    let password = fragment
        .map(decode)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Some(Entry {
        scheme,
        authority,
        id,
        password,
    })
}

/// The single entry identifier a path names, lowercase; `None` for anything else.
fn entry_id(path: &str) -> Option<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    if !ID_LENGTH.contains(&trimmed.len()) || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Whether `authority` is one of the two live service domains, with or without `www.` and
/// with or without a port.
fn is_claimed_host(authority: &str) -> bool {
    CLAIMED_HOSTS.contains(&bare_host(authority).as_str())
}

/// Whether `url` points back at the service itself rather than at a hoster.
#[must_use]
pub fn is_service_url(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    SERVICE_HOSTS.contains(&bare_host(&rest[..end].to_ascii_lowercase()).as_str())
}

/// A host without its port and without a leading `www.`, lowercase.
fn bare_host(authority: &str) -> String {
    let host = authority.split(':').next().unwrap_or(authority);
    host.strip_prefix("www.")
        .unwrap_or(host)
        .to_ascii_lowercase()
}

/// Whether the service answered somewhere other than the entry that was asked for.
///
/// A deleted entry answers `200` at the front page rather than `404` — measured on
/// `peeplink.in/0005738dc976` — so the status alone does not tell the two apart. Only the
/// identifier is compared, because `www.peeplink.in/<id>` legitimately redirects to
/// `peeplink.in/<id>` and that is the same entry.
#[must_use]
pub fn redirected_off_entry(entry: &Entry, final_url: &str) -> bool {
    let Some((_, rest)) = final_url.split_once("://") else {
        // An answer without a usable address is not evidence that the entry moved.
        return false;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let path = &rest[end..];
    let path = path.split(['?', '#']).next().unwrap_or("");
    entry_id(path).as_deref() != Some(entry.id.as_str())
}

/// Percent-decodes one piece of an address, leaving anything malformed as it was.
#[must_use]
pub fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
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

/// Percent-encodes one value for an `application/x-www-form-urlencoded` body.
///
/// An allowlist rather than a denylist: the value is an access password somebody typed, and
/// guessing which bytes this service tolerates unencoded is how a password arrives wrong.
#[must_use]
pub fn encode_form_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Entry, claim, encode_form_value, is_service_url, redirected_off_entry};

    /// The two live domains and the shape of an identifier, and nothing else.
    #[test]
    fn only_the_two_live_domains_with_an_entry_identifier_are_claimed() {
        for claimed in [
            "https://peeplink.in/0004ae96cef6",
            "https://www.peeplink.in/0004ae96cef6",
            "http://peeplink.in/00013b965394",
            "https://alfalink.to/02489255ba1048ae9d1328",
            "https://www.alfalink.to/13e2cd9a35efcd6c6e4766",
            // A trailing slash is the same entry.
            "https://peeplink.in/0004ae96cef6/",
        ] {
            assert!(claim(claimed).is_some(), "{claimed}");
        }
        for left_alone in [
            // The front page, the terms, the login: not entries.
            "https://peeplink.in/",
            "https://peeplink.in/tos.html",
            "https://peeplink.in/index.php",
            "https://alfalink.to/login.php",
            // The dead alias is recognised as the service but never claimed: the manifest
            // grants no request to it.
            "https://alfalink.info/02489255ba1048ae9d1328",
            // Somebody else's site with the same shape.
            "https://example.org/0004ae96cef6",
            // Not hexadecimal, and too short.
            "https://peeplink.in/zzzzzzzzzzzz",
            "https://peeplink.in/00013b",
            // Two segments are not one identifier.
            "https://peeplink.in/0004ae96cef6/extra",
            "ftp://peeplink.in/0004ae96cef6",
            "magnet:?xt=urn:btih:abc",
        ] {
            assert!(claim(left_alone).is_none(), "{left_alone}");
        }
    }

    /// The identifier is lowercased and the address to request carries no fragment.
    #[test]
    fn an_entry_address_is_rebuilt_without_the_fragment() {
        let entry = claim("https://PeepLink.in/0004AE96CEF6#s3cret").expect("entry");
        assert_eq!(entry.authority, "peeplink.in");
        assert_eq!(entry.id, "0004ae96cef6");
        assert_eq!(entry.to_url(), "https://peeplink.in/0004ae96cef6");
        assert_eq!(entry.password.as_deref(), Some("s3cret"));
    }

    /// The password rides in the fragment, which is never sent to a server, and is decoded.
    #[test]
    fn a_password_after_the_hash_is_read_and_decoded() {
        let entry = claim("https://peeplink.in/0004ae96cef6#let%20me%20in").expect("entry");
        assert_eq!(entry.password.as_deref(), Some("let me in"));
        let bare = claim("https://peeplink.in/0004ae96cef6").expect("entry");
        assert_eq!(bare.password, None);
        let empty = claim("https://peeplink.in/0004ae96cef6#%20").expect("entry");
        assert_eq!(empty.password, None);
    }

    /// A deleted entry answers `200` at the front page, so only the identifier decides.
    #[test]
    fn a_redirect_to_the_front_page_is_seen_and_one_between_domains_is_not() {
        let entry = claim("https://www.peeplink.in/0005738dc976").expect("entry");
        assert!(redirected_off_entry(&entry, "https://peeplink.in/"));
        assert!(redirected_off_entry(
            &entry,
            "https://peeplink.in/get_links.html"
        ));
        // The service's own `www.` redirect keeps the entry; that is not a deletion.
        assert!(!redirected_off_entry(
            &entry,
            "https://peeplink.in/0005738dc976"
        ));
        assert!(!redirected_off_entry(
            &entry,
            "https://peeplink.in/0005738dc976?x=1"
        ));
        // An answer without a usable address proves nothing either way.
        assert!(!redirected_off_entry(&entry, "not a url"));
    }

    /// The service's own addresses are not downloads, the dead alias included.
    #[test]
    fn the_services_own_links_are_recognised_including_the_dead_alias() {
        for own in [
            "https://peeplink.in/",
            "https://www.peeplink.in/0004ae96cef6",
            "https://alfalink.to/tos.html",
            "https://alfalink.info/02489255ba1048ae9d1328",
        ] {
            assert!(is_service_url(own), "{own}");
        }
        for foreign in [
            "https://rapidgator.net/file/abc/x.rar.html",
            "http://uploaded.net/file/1fnqt1le/x.rar",
            "https://t.me/FPE20",
            "not a url",
        ] {
            assert!(!is_service_url(foreign), "{foreign}");
        }
    }

    /// The password travels in a form body, so everything that is not unreserved is encoded.
    #[test]
    fn a_password_is_encoded_for_a_form_body() {
        assert_eq!(encode_form_value("plain-1_a.b~c"), "plain-1_a.b~c");
        assert_eq!(encode_form_value("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode_form_value("\u{e4}"), "%C3%A4");
        assert_eq!(encode_form_value(""), "");
    }

    /// The struct is built by `claim` and nowhere else; this keeps its shape honest.
    #[test]
    fn an_entry_is_only_what_claim_builds() {
        let built = Entry {
            scheme: "https".to_owned(),
            authority: "alfalink.to".to_owned(),
            id: "02489255ba1048ae9d1328".to_owned(),
            password: None,
        };
        assert_eq!(
            claim("https://alfalink.to/02489255ba1048ae9d1328").expect("entry"),
            built
        );
        assert_eq!(built.to_url(), "https://alfalink.to/02489255ba1048ae9d1328");
    }
}

//! Reading a Google address without a URL parser.
//!
//! Shared because the resolver and the crawler have to agree on it exactly. They claim
//! *different* addresses — one files, one folders — but they must agree on which hosts are
//! Google's, on what a Drive file id may look like, and on the `/u/<n>` account prefix Google
//! puts in front of nearly every link of its own. Two implementations of that would eventually
//! be two answers, and the one that mattered would be "is `drive.google.com.evil.test` Google".
//!
//! Written without a URL parser so the same code compiles into a component, where the `url`
//! crate is not part of the guest build.

/// The Google hosts these plugins know. A host merely *ending* in the same letters is not one
/// of them, which is what `drive.google.com.evil.test` would be.
pub const HOSTS: [&str; 4] = [
    "drive.google.com",
    "docs.google.com",
    "drive.usercontent.google.com",
    "www.googleapis.com",
];

/// Which of [`HOSTS`] this is, or `None`.
#[must_use]
pub fn google_host(host: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    HOSTS.into_iter().find(|known| *known == host)
}

/// Scheme, host and the rest of an address.
///
/// Returns `None` for anything that is not plain http(s), and for an authority carrying
/// credentials — accepting those would let `x@evil.test` read as one of Google's own hosts.
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

/// The value of one query parameter, undecoded.
#[must_use]
pub fn query_value<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// The path segments of an address, with the `/u/<n>` account prefix removed.
#[must_use]
pub fn segments(route: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = route.split('/').filter(|part| !part.is_empty()).collect();
    let has_account_prefix = parts.first() == Some(&"u")
        && parts
            .get(1)
            .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()));
    if has_account_prefix {
        parts.drain(..2);
    }
    parts
}

/// A Drive file or folder id as the API issues them: URL-safe base64 characters and nothing
/// else.
///
/// Bounded and character-checked rather than trusted, because it is pasted straight into a
/// request path. Anything else could be a path segment, an escape or a second address.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The canonical address for one Drive item, and the one both plugins hand each other.
///
/// This is the whole of the contract between the crawler and the resolver: the crawler answers
/// with these, the resolver claims exactly these, and neither needs to know the other exists.
/// A private channel between two signed packages would be a worse version of the same thing.
#[must_use]
pub fn file_address(id: &str) -> String {
    format!("https://drive.google.com/file/d/{id}/view")
}

/// Percent-encodes a value for an address a plugin builds itself.
///
/// Needed where the finished address goes back to the host as a string rather than as a query
/// list the host would encode — the Workspace export `mimeType`, which contains `/` and `+`.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        file_address, google_host, percent_encode, query_value, segments, split, valid_id,
    };

    #[test]
    fn only_googles_own_hosts_are_googles() {
        assert_eq!(google_host("drive.google.com"), Some("drive.google.com"));
        assert_eq!(google_host("DRIVE.GOOGLE.COM."), Some("drive.google.com"));
        assert_eq!(google_host("drive.google.com.evil.test"), None);
        assert_eq!(google_host("evil-drive.google.com"), None);
        assert_eq!(google_host("google.com"), None);
    }

    #[test]
    fn an_authority_carrying_credentials_is_refused() {
        assert_eq!(split("https://x@drive.google.com/a"), None);
        assert_eq!(split("ftp://drive.google.com/a"), None);
        assert_eq!(
            split("https://drive.google.com:443/a/b?c=d#e"),
            Some(("drive.google.com", "a/b?c=d"))
        );
    }

    #[test]
    fn the_account_prefix_google_puts_in_its_own_links_is_stripped() {
        assert_eq!(segments("/u/0/folders/abc"), vec!["folders", "abc"]);
        assert_eq!(
            segments("/u/12/document/d/abc"),
            vec!["document", "d", "abc"]
        );
        // Not a prefix: `u` followed by something that is not a number is a path of its own.
        assert_eq!(segments("/u/name/x"), vec!["u", "name", "x"]);
        assert_eq!(
            segments("/file/d/abc/view"),
            vec!["file", "d", "abc", "view"]
        );
    }

    #[test]
    fn a_query_value_is_read_without_decoding_it() {
        assert_eq!(query_value("open?id=abc&x=1", "id"), Some("abc"));
        assert_eq!(query_value("open?x=1&id=abc", "id"), Some("abc"));
        assert_eq!(query_value("open", "id"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert!(valid_id("1A2b3C4d-5E6f_7G8h"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a%2Fb"));
        assert!(!valid_id(".."));
        assert!(!valid_id(&"x".repeat(129)));
    }

    /// The one address the crawler and the resolver hand each other.
    #[test]
    fn the_canonical_address_is_one_the_resolver_claims() {
        assert_eq!(
            file_address("1A2b"),
            "https://drive.google.com/file/d/1A2b/view"
        );
    }

    #[test]
    fn an_export_mime_type_survives_being_put_in_an_address() {
        assert_eq!(
            percent_encode("application/vnd.google-apps.script+json"),
            "application%2Fvnd.google-apps.script%2Bjson"
        );
        assert_eq!(percent_encode("application/pdf"), "application%2Fpdf");
    }
}

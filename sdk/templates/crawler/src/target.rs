//! Deciding whether an address is one this plugin knows how to open.
//!
//! Kept apart from the component, and written without a URL parser, so it runs on the host
//! target as it is: `claims-url` is the one call a crawler makes on every link a person
//! pastes, and it has to be both cheap and provably narrow.

/// What an address points at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A directory, to be listed and walked.
    Folder,
    /// A single item, which still has to be looked up to learn its address and size.
    Item,
}

/// An address this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub kind: Kind,
    pub id: String,
}

/// The scheme, host and path of an address, without pulling in a URL parser.
fn split(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let rest = rest.split(['#']).next()?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // Credentials in an address are not something a share link carries, and accepting them
    // would let `user@evil.example` read as this plugin's own host.
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    Some((scheme, host, path))
}

/// Whether `host` is `expected` or a subdomain of it, compared case-insensitively.
fn host_matches(host: &str, expected: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == expected || host.ends_with(&format!(".{expected}"))
}

/// The value of one query parameter, undecoded beyond `+`.
fn query_value<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// An identifier is opaque to this plugin, so it is accepted only in the narrow shape the
/// provider issues. Anything else could be a path, an escape or a second address.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Reads an address, returning what this plugin would open — or `None`, which is the answer
/// `claims-url` gives for everything that is not this provider's folder or item address.
///
/// Both spellings providers use are accepted: the identifier as the last path segment, and
/// the identifier as a query parameter.
#[must_use]
pub fn claim(url: &str, expected_host: &str) -> Option<Target> {
    let (_, host, path) = split(url)?;
    if !host_matches(host, expected_host) {
        return None;
    }
    let route = path.split('?').next()?;
    let mut segments = route.split('/').filter(|segment| !segment.is_empty());
    let kind = match segments.next()? {
        "folder" => Kind::Folder,
        "item" => Kind::Item,
        _ => return None,
    };
    let id = match segments.next() {
        Some(segment) => segment,
        None => query_value(path, "id")?,
    };
    // A third segment means this is some other page that merely starts the same way.
    if segments.next().is_some() {
        return None;
    }
    valid_id(id).then(|| Target {
        kind,
        id: id.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Kind, Target, claim};

    const HOST: &str = "api.example.com";

    #[test]
    fn both_spellings_of_a_folder_address_are_claimed() {
        assert_eq!(
            claim("https://api.example.com/folder/abc123", HOST),
            Some(Target {
                kind: Kind::Folder,
                id: "abc123".to_owned()
            })
        );
        assert_eq!(
            claim("https://api.example.com/folder?id=abc123", HOST),
            Some(Target {
                kind: Kind::Folder,
                id: "abc123".to_owned()
            })
        );
        assert_eq!(
            claim("https://www.api.example.com/item/x-1_2", HOST).map(|target| target.kind),
            Some(Kind::Item)
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        // The failure that hurts: a crawler answering yes to everything makes every pasted
        // link wait for a folder listing that will never come.
        assert_eq!(claim("https://cdn.example.org/folder/abc", HOST), None);
        assert_eq!(
            claim("https://api.example.com.evil.test/folder/a", HOST),
            None
        );
        assert_eq!(claim("https://user@api.example.com/folder/a", HOST), None);
        assert_eq!(claim("ftp://api.example.com/folder/a", HOST), None);
        assert_eq!(claim("https://api.example.com/account/info", HOST), None);
        assert_eq!(claim("https://api.example.com/folder", HOST), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(
            claim("https://api.example.com/folder/../../etc", HOST),
            None
        );
        assert_eq!(claim("https://api.example.com/folder/a%2Fb", HOST), None);
        assert_eq!(
            claim(
                &format!("https://api.example.com/folder/{}", "x".repeat(65)),
                HOST
            ),
            None
        );
    }
}

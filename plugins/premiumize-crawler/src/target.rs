//! Deciding whether an address is a Premiumize folder or item.
//!
//! `claims-url` is asked of every link a person pastes, so this runs often, reaches nothing
//! and answers from the address alone. It is also the place over-claiming would hurt: the
//! resolver already claims every http(s) address, because a multihoster is chosen by
//! catalogue rather than by domain. A crawler that did the same would try to list a folder
//! behind every link in the queue.

/// What a Premiumize address points at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A cloud folder, to be listed and walked with `folder/list`.
    Folder,
    /// A single cloud item, looked up with `item/details`.
    Item,
}

/// An address this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub kind: Kind,
    pub id: String,
}

/// Premiumize's own hosts. A subdomain is accepted; a host merely ending in the same letters
/// is not, which is what `premiumize.me.evil.test` would be.
fn is_premiumize(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "premiumize.me" || host.ends_with(".premiumize.me")
}

/// Scheme, host and the rest of an address, without a URL parser.
fn split(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let rest = rest.split('#').next()?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // A share link carries no credentials, and accepting them would let `x@evil.test` read
    // as this plugin's own host.
    if authority.contains('@') {
        return None;
    }
    Some((authority.split(':').next()?, path))
}

/// The value of one query parameter.
fn query_value<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// A Premiumize identifier is opaque to this plugin, so it is accepted only in the narrow
/// shape the API issues. Anything else could be a path, an escape or a second address.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Reads an address, returning what this plugin would open — or `None`, which is what
/// `claims-url` answers for everything that is not a Premiumize folder or item.
///
/// Three spellings are in use: the share path `/folder/<id>`, the web application's
/// `/files?folder_id=<id>`, and the same two for a single item.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    let (host, path) = split(url)?;
    if !is_premiumize(host) {
        return None;
    }
    let route = path.split('?').next()?;
    let mut segments = route.split('/').filter(|segment| !segment.is_empty());
    let (kind, query_key) = match segments.next().unwrap_or_default() {
        "folder" => (Kind::Folder, "id"),
        "item" => (Kind::Item, "id"),
        "files" => (Kind::Folder, "folder_id"),
        _ => return None,
    };
    let id = match segments.next() {
        Some(segment) => segment,
        None => query_value(path, query_key)?,
    };
    // A third segment means some other page that merely starts the same way.
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

    #[test]
    fn the_three_spellings_of_a_folder_address_are_claimed() {
        let folder = Some(Target {
            kind: Kind::Folder,
            id: "abc123".to_owned(),
        });
        assert_eq!(claim("https://www.premiumize.me/folder/abc123"), folder);
        assert_eq!(claim("https://premiumize.me/folder?id=abc123"), folder);
        assert_eq!(
            claim("https://www.premiumize.me/files?folder_id=abc123&x=1"),
            folder
        );
        assert_eq!(
            claim("https://www.premiumize.me/item/abc123"),
            Some(Target {
                kind: Kind::Item,
                id: "abc123".to_owned()
            })
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        // The resolver claims every http(s) address because a multihoster is chosen by
        // catalogue. A crawler doing the same would list a folder behind every link.
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://premiumize.me.evil.test/folder/a"), None);
        assert_eq!(claim("https://x@www.premiumize.me/folder/a"), None);
        assert_eq!(claim("ftp://www.premiumize.me/folder/a"), None);
        assert_eq!(claim("https://www.premiumize.me/account"), None);
        assert_eq!(claim("https://www.premiumize.me/"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(claim("https://www.premiumize.me/folder/../../api"), None);
        assert_eq!(claim("https://www.premiumize.me/folder/a%2Fb"), None);
        assert_eq!(claim("https://www.premiumize.me/folder"), None);
        assert_eq!(
            claim(&format!(
                "https://www.premiumize.me/folder/{}",
                "x".repeat(65)
            )),
            None
        );
    }
}

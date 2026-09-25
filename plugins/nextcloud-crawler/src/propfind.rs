//! Reading a WebDAV `PROPFIND` answer.
//!
//! `crates/rd-webdav/src/propfind.rs` is the model for what is read out of the document, not
//! a dependency: a plugin component links nothing outside `rdownloader:plugin`, so the parser
//! is here, small, and tested against the documents Nextcloud and ownCloud actually send.
//!
//! The scanner works on local names — `<d:response>`, `<D:response>` and `<response>` are the
//! same element — because the namespace prefix is the server's choice and has changed between
//! Nextcloud releases.

/// The `PROPFIND` body: exactly the three properties a listing needs.
///
/// Asking for everything (`<d:allprop/>`) is the other option and a much larger answer for a
/// wide folder, which is the one thing the response budget cannot absorb.
pub const BODY: &str = concat!(
    r#"<?xml version="1.0" encoding="utf-8"?>"#,
    r#"<d:propfind xmlns:d="DAV:"><d:prop>"#,
    r#"<d:displayname/><d:getcontentlength/><d:resourcetype/>"#,
    r#"</d:prop></d:propfind>"#
);

/// One entry the server named.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    /// The address the server gave, as it gave it: a path, percent-encoded.
    pub href: String,
    /// The name to show, from `displayname` or, failing that, from the address.
    pub name: String,
    pub size: Option<u64>,
    pub is_collection: bool,
}

/// Every entry in a multistatus document.
///
/// The first entry of a `Depth: 1` answer is the collection that was asked about; the caller
/// drops it by comparing addresses rather than by position, because a server that answers in
/// another order would otherwise lose a file and gain a loop.
#[must_use]
pub fn items(xml: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for (start, end) in elements(xml, "response") {
        let inner = &xml[start..end];
        let Some(href) = text(inner, "href") else {
            continue;
        };
        // A property the server could not produce comes back in a `404` propstat; only the
        // ones it did produce are read, and `getcontentlength` is absent for a collection.
        let is_collection = !elements(inner, "collection").is_empty();
        let size = text(inner, "getcontentlength").and_then(|value| value.trim().parse().ok());
        let name = text(inner, "displayname")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| name_from_href(&href));
        out.push(Item {
            href: href.trim().to_owned(),
            name,
            size: if is_collection { None } else { size },
            is_collection,
        });
    }
    out
}

/// The name an address ends in, percent-decoded.
fn name_from_href(href: &str) -> String {
    let trimmed = href.trim().trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or_default();
    crate::target::decode(last)
}

/// The text of the first element with this local name, with the five XML entities undone.
fn text(xml: &str, name: &str) -> Option<String> {
    let (start, end) = elements(xml, name).into_iter().next()?;
    Some(unescape(&xml[start..end]))
}

/// The content ranges of every element with this local name, ignoring namespace prefixes.
///
/// A self-closing element yields an empty range, which is what makes `<d:collection/>` — the
/// marker that says "this is a folder" — findable at all.
fn elements(xml: &str, name: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = xml[cursor..].find('<') {
        let open = cursor + offset;
        let rest = &xml[open + 1..];
        if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
            cursor = open + 1;
            continue;
        }
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[..close];
        let local = tag
            .split(|character: char| character.is_whitespace() || character == '/')
            .next()
            .unwrap_or("")
            .rsplit(':')
            .next()
            .unwrap_or("");
        let inner_start = open + 1 + close + 1;
        if !local.eq_ignore_ascii_case(name) {
            cursor = open + 1;
            continue;
        }
        if tag.ends_with('/') {
            out.push((inner_start, inner_start));
            cursor = inner_start;
            continue;
        }
        match closing(xml, inner_start, name) {
            Some(closes_at) => {
                out.push((inner_start, closes_at));
                cursor = closes_at;
            }
            None => cursor = inner_start,
        }
    }
    out
}

/// Where the closing tag for this local name starts, searching from `from`.
fn closing(xml: &str, from: usize, name: &str) -> Option<usize> {
    let mut cursor = from;
    while let Some(offset) = xml[cursor..].find("</") {
        let open = cursor + offset;
        let rest = &xml[open + 2..];
        let end = rest.find('>')?;
        let local = rest[..end].trim().rsplit(':').next().unwrap_or("");
        if local.eq_ignore_ascii_case(name) {
            return Some(open);
        }
        cursor = open + 2;
    }
    None
}

/// The five entities XML defines, and nothing else: a numeric reference is left as it stands
/// rather than decoded into a character a name has no business carrying.
fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::{Item, items};

    /// A `Depth: 1` answer from Nextcloud 29 on the public DAV endpoint.
    const NEXTCLOUD: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns">
 <d:response>
  <d:href>/public.php/dav/files/abcdefghijklmno/</d:href>
  <d:propstat><d:prop><d:displayname>Holiday &amp; more</d:displayname>
   <d:resourcetype><d:collection/></d:resourcetype></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><d:getcontentlength/></d:prop>
   <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
 </d:response>
 <d:response>
  <d:href>/public.php/dav/files/abcdefghijklmno/Season%201/</d:href>
  <d:propstat><d:prop><d:displayname>Season 1</d:displayname>
   <d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
 </d:response>
 <d:response>
  <d:href>/public.php/dav/files/abcdefghijklmno/disc.iso</d:href>
  <d:propstat><d:prop><d:displayname>disc.iso</d:displayname>
   <d:getcontentlength>1048576</d:getcontentlength>
   <d:resourcetype/></d:prop></d:propstat>
 </d:response>
</d:multistatus>"#;

    /// ownCloud 10 on the older endpoint: a different prefix, no `displayname`.
    const OWNCLOUD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
 <D:response><D:href>/public.php/webdav/</D:href>
  <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
 </D:response>
 <D:response><D:href>/public.php/webdav/notes%20and%20more.txt</D:href>
  <D:propstat><D:prop><D:getcontentlength>42</D:getcontentlength>
  <D:resourcetype/></D:prop></D:propstat>
 </D:response>
</D:multistatus>"#;

    #[test]
    fn a_nextcloud_listing_becomes_its_entries_with_names_and_sizes() {
        let found = items(NEXTCLOUD);
        assert_eq!(found.len(), 3);
        assert_eq!(
            found[0],
            Item {
                href: "/public.php/dav/files/abcdefghijklmno/".to_owned(),
                name: "Holiday & more".to_owned(),
                size: None,
                is_collection: true,
            },
            "the collection itself, with its entities undone"
        );
        assert!(found[1].is_collection, "a subfolder is a collection");
        assert_eq!(found[1].name, "Season 1");
        assert_eq!(
            found[2],
            Item {
                href: "/public.php/dav/files/abcdefghijklmno/disc.iso".to_owned(),
                name: "disc.iso".to_owned(),
                size: Some(1_048_576),
                is_collection: false,
            }
        );
    }

    /// The prefix is the server's choice, and ownCloud names nothing: the address has to be
    /// enough for a name.
    #[test]
    fn an_owncloud_listing_is_read_through_a_different_prefix() {
        let found = items(OWNCLOUD);
        assert_eq!(found.len(), 2);
        assert!(found[0].is_collection);
        assert_eq!(found[1].name, "notes and more.txt");
        assert_eq!(found[1].size, Some(42));
        assert!(!found[1].is_collection);
    }

    /// Anything that is not a multistatus document is no entries, not a panic.
    #[test]
    fn a_page_that_is_not_a_listing_yields_nothing() {
        assert!(items("<html><body>Sign in</body></html>").is_empty());
        assert!(items("").is_empty());
        assert!(items("<d:multistatus><d:response>").is_empty());
    }
}

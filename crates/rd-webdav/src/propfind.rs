//! Parsing a WebDAV `multistatus` response into the reviewable [`RemoteListing`].
//!
//! The parser is written against the raw event stream rather than serde, because a
//! multistatus body is namespace-heavy, servers disagree about prefixes, and the entry
//! count has to be bounded while parsing rather than afterwards.

use chrono::{DateTime, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;
use rd_core::{
    ByteCount, ListingLimit, MAX_REMOTE_ENTRIES, RemoteEntry, RemoteListing, is_safe_relative_path,
};
use url::Url;

/// Largest multistatus body accepted, so a hostile or broken server cannot exhaust memory.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Deepest element nesting accepted. A legitimate multistatus is about six levels deep;
/// anything far beyond that is a nesting attack rather than a listing.
const MAX_DEPTH: usize = 64;

/// The `PROPFIND` request body: exactly the properties that are used, so servers do not
/// have to serialise every dead property they know about.
pub const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getlastmodified/>
    <D:getetag/>
  </D:prop>
</D:propfind>"#;

/// Why a multistatus body was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// Not well-formed, or not a multistatus at all.
    Malformed,
    /// Nested far deeper than any real listing.
    TooDeep,
    /// A `href` resolved outside the collection that was asked for.
    EscapesRoot { href: String },
}

/// One `<response>` from the body.
#[derive(Clone, Debug)]
struct Response {
    href: String,
    is_collection: bool,
    length: Option<u64>,
    last_modified: Option<DateTime<Utc>>,
    etag: Option<String>,
}

/// Parses a multistatus body into a listing relative to `base`.
///
/// `base` is the collection that was asked for; every `href` must resolve underneath it.
/// A server that answers with a href elsewhere is either broken or trying to make the
/// client write outside the folder it agreed to, and neither is worth guessing about.
pub fn parse(body: &str, base: &Url) -> Result<RemoteListing, ParseError> {
    let responses = read_responses(body)?;
    let base_path = normalize_path(base.path());
    let mut entries = Vec::new();
    let mut truncated = None;
    let mut self_entry = None;

    for response in responses {
        let href = resolve(base, &response.href)?;
        let path = normalize_path(href.path());
        // The collection describes itself in its own listing; that is the entry that says
        // whether the link was a file or a folder, not a child of it.
        if path == base_path {
            self_entry = Some(response);
            continue;
        }
        let Some(relative) = path.strip_prefix(&format!("{}/", base_path.trim_end_matches('/')))
        else {
            return Err(ParseError::EscapesRoot {
                href: response.href.clone(),
            });
        };
        let relative = relative.trim_end_matches('/').to_owned();
        if relative.is_empty() {
            continue;
        }
        if !is_safe_relative_path(&relative) {
            return Err(ParseError::EscapesRoot {
                href: response.href.clone(),
            });
        }
        if entries.len() >= MAX_REMOTE_ENTRIES {
            truncated = Some(ListingLimit::EntryCount);
            break;
        }
        entries.push(RemoteEntry {
            path: relative,
            is_dir: response.is_collection,
            size: response.length.and_then(|value| ByteCount::new(value).ok()),
            modified: response.last_modified,
            etag: response.etag,
        });
    }

    // Depth 1 on a file returns exactly one response: the file itself.
    let single_file = entries.is_empty()
        && self_entry
            .as_ref()
            .is_some_and(|response| !response.is_collection);
    if single_file {
        let response = self_entry.expect("checked above");
        let name = file_name(&normalize_path(resolve(base, &response.href)?.path()));
        if !is_safe_relative_path(&name) {
            return Err(ParseError::EscapesRoot {
                href: response.href,
            });
        }
        return Ok(RemoteListing {
            root: parent_path(&base_path),
            single_file: true,
            entries: vec![RemoteEntry {
                path: name,
                is_dir: false,
                size: response.length.and_then(|value| ByteCount::new(value).ok()),
                modified: response.last_modified,
                etag: response.etag,
            }],
            truncated: None,
            // Filled in by the caller from the `Accept-Ranges` header; PROPFIND says
            // nothing about whether a GET can be resumed.
            supports_resume: false,
        });
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(RemoteListing {
        root: base_path,
        single_file: false,
        entries,
        truncated,
        supports_resume: false,
    })
}

/// Walks the event stream and collects the `<response>` elements.
///
/// `quick_xml` does not expand entities unless asked to, which is what keeps a billion
/// laughs or an external entity out of this parser; a DTD is skipped outright rather than
/// being interpreted.
fn read_responses(body: &str) -> Result<Vec<Response>, ParseError> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    // Namespace prefixes vary per server (`D:`, `d:`, none at all), so elements are matched
    // on their local name.
    let mut depth = 0usize;
    let mut responses = Vec::new();
    let mut current: Option<Response> = None;
    let mut in_href = false;
    let mut property: Option<Property> = None;
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(ParseError::TooDeep);
                }
                match local_name(start.name().as_ref()) {
                    b"response" => current = Some(Response::empty()),
                    b"href" if current.is_some() => in_href = true,
                    b"collection" => {
                        if let Some(response) = current.as_mut() {
                            response.is_collection = true;
                        }
                    }
                    b"getcontentlength" => property = Some(Property::Length),
                    b"getlastmodified" => property = Some(Property::LastModified),
                    b"getetag" => property = Some(Property::Etag),
                    _ => {}
                }
            }
            Ok(Event::Empty(empty)) => {
                // `<D:collection/>` is the usual way a server marks a folder.
                if local_name(empty.name().as_ref()) == b"collection"
                    && let Some(response) = current.as_mut()
                {
                    response.is_collection = true;
                }
            }
            Ok(Event::Text(text)) => {
                let value = text.decode().map_err(|_| ParseError::Malformed)?;
                let value = value.trim();
                if value.is_empty() {
                } else if in_href {
                    if let Some(response) = current.as_mut() {
                        response.href = value.to_owned();
                    }
                } else if let (Some(kind), Some(response)) = (property, current.as_mut()) {
                    kind.apply(response, value);
                }
            }
            Ok(Event::End(end)) => {
                depth = depth.saturating_sub(1);
                match local_name(end.name().as_ref()) {
                    b"response" => {
                        if let Some(response) = current.take()
                            && !response.href.is_empty()
                        {
                            responses.push(response);
                        }
                    }
                    b"href" => in_href = false,
                    b"getcontentlength" | b"getlastmodified" | b"getetag" => property = None,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            // A DTD is never processed. Entity expansion is where XML parsers turn a
            // listing into a denial of service or a file read.
            Ok(Event::DocType(_)) => return Err(ParseError::Malformed),
            Ok(_) => {}
            Err(_) => return Err(ParseError::Malformed),
        }
        buffer.clear();
    }
    if responses.is_empty() {
        return Err(ParseError::Malformed);
    }
    Ok(responses)
}

#[derive(Clone, Copy)]
enum Property {
    Length,
    LastModified,
    Etag,
}

impl Property {
    fn apply(self, response: &mut Response, value: &str) {
        match self {
            Self::Length => response.length = value.parse().ok(),
            Self::LastModified => {
                response.last_modified = httpdate::parse_http_date(value)
                    .ok()
                    .map(DateTime::<Utc>::from);
            }
            // A weak validator says the body may differ while the tag is unchanged, so it
            // cannot decide whether a partial file is still valid. Treating it as absent
            // makes the resume fall back to size and modification time.
            Self::Etag => response.etag = (!is_weak(value)).then(|| value.trim().to_owned()),
        }
    }
}

/// Whether an ETag is marked weak (`W/"..."`).
#[must_use]
pub fn is_weak(etag: &str) -> bool {
    let trimmed = etag.trim_start();
    trimmed.starts_with("W/") || trimmed.starts_with("w/")
}

impl Response {
    const fn empty() -> Self {
        Self {
            href: String::new(),
            is_collection: false,
            length: None,
            last_modified: None,
            etag: None,
        }
    }
}

/// Strips the namespace prefix from an element name.
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|byte| *byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

/// Resolves a `href` against the collection URL, refusing anything on another origin.
fn resolve(base: &Url, href: &str) -> Result<Url, ParseError> {
    let resolved = base.join(href).map_err(|_| ParseError::Malformed)?;
    if resolved.origin() != base.origin() {
        return Err(ParseError::EscapesRoot {
            href: href.to_owned(),
        });
    }
    Ok(resolved)
}

/// Percent-decodes a URL path and removes a trailing slash.
fn normalize_path(path: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_or_else(|_| path.to_owned(), std::borrow::Cow::into_owned);
    let trimmed = decoded.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn parent_path(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => path[..index].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_DEPTH, ParseError, is_weak, parse};
    use url::Url;

    fn base(path: &str) -> Url {
        Url::parse(&format!("https://cloud.example{path}")).expect("url")
    }

    const COLLECTION: &str = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/share/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
    <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/share/movie.mkv</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
      <D:getcontentlength>1234</D:getcontentlength>
      <D:getlastmodified>Sat, 10 Jan 2026 12:00:00 GMT</D:getlastmodified>
      <D:getetag>"abc123"</D:getetag>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/share/extras/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
    <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;

    #[test]
    fn a_collection_lists_its_children_without_itself() {
        let listing = parse(COLLECTION, &base("/dav/share/")).expect("parse");
        assert!(!listing.single_file);
        assert_eq!(listing.root, "/dav/share");
        let paths: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, ["extras", "movie.mkv"]);
        let movie = &listing.entries[1];
        assert_eq!(movie.size.map(rd_core::ByteCount::get), Some(1234));
        assert_eq!(movie.etag.as_deref(), Some("\"abc123\""));
        assert!(movie.modified.is_some());
        assert!(listing.entries[0].is_dir);
    }

    #[test]
    fn a_file_resolves_as_a_single_entry() {
        let body = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/share/movie.mkv</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
      <D:getcontentlength>99</D:getcontentlength>
    </D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;
        let listing = parse(body, &base("/dav/share/movie.mkv")).expect("parse");
        assert!(listing.single_file);
        assert_eq!(listing.root, "/dav/share");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].path, "movie.mkv");
        assert_eq!(
            listing.entries[0].size.map(rd_core::ByteCount::get),
            Some(99)
        );
    }

    #[test]
    fn namespace_prefixes_do_not_matter() {
        // Servers use `D:`, `d:` or no prefix at all; matching is on local names.
        let body = COLLECTION.replace("D:", "d:");
        let listing = parse(&body, &base("/dav/share/")).expect("parse");
        assert_eq!(listing.entries.len(), 2);
    }

    #[test]
    fn percent_encoded_hrefs_are_decoded() {
        let body = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/share/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response><D:href>/dav/share/my%20movie.mkv</D:href>
    <D:propstat><D:prop><D:getcontentlength>5</D:getcontentlength></D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;
        let listing = parse(body, &base("/dav/share/")).expect("parse");
        assert_eq!(listing.entries[0].path, "my movie.mkv");
    }

    #[test]
    fn a_doctype_is_refused_rather_than_expanded() {
        // The bug this locks in: expanding entities turns a listing into a billion-laughs
        // denial of service or an external file read.
        let body = r#"<?xml version="1.0"?>
<!DOCTYPE multistatus [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/share/&xxe;</D:href></D:response>
</D:multistatus>"#;
        assert!(matches!(
            parse(body, &base("/dav/share/")),
            Err(ParseError::Malformed)
        ));
    }

    #[test]
    fn an_href_outside_the_collection_is_refused() {
        // A traversing href would otherwise decide where a file is written.
        for href in [
            "/etc/passwd",
            "/dav/other/file.bin",
            "https://elsewhere.example/dav/share/file.bin",
        ] {
            let body = format!(
                r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/share/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response><D:href>{href}</D:href>
    <D:propstat><D:prop><D:getcontentlength>5</D:getcontentlength></D:prop></D:propstat>
  </D:response>
</D:multistatus>"#
            );
            assert!(
                matches!(
                    parse(&body, &base("/dav/share/")),
                    Err(ParseError::EscapesRoot { .. })
                ),
                "{href} should have been refused"
            );
        }
    }

    #[test]
    fn a_deeply_nested_body_is_refused() {
        let body = format!(
            "<D:multistatus xmlns:D=\"DAV:\">{}{}</D:multistatus>",
            "<a>".repeat(MAX_DEPTH + 5),
            "</a>".repeat(MAX_DEPTH + 5)
        );
        assert!(matches!(
            parse(&body, &base("/dav/share/")),
            Err(ParseError::TooDeep)
        ));
    }

    #[test]
    fn a_weak_etag_is_treated_as_absent() {
        // A weak validator allows the body to differ while the tag stays the same, so it
        // cannot decide whether a partial file is still valid.
        assert!(is_weak("W/\"abc\""));
        assert!(is_weak("w/\"abc\""));
        assert!(!is_weak("\"abc\""));
        let body = COLLECTION.replace("<D:getetag>\"abc123\"", "<D:getetag>W/\"abc123\"");
        let listing = parse(&body, &base("/dav/share/")).expect("parse");
        let movie = listing
            .entries
            .iter()
            .find(|entry| entry.path == "movie.mkv")
            .expect("movie");
        assert_eq!(movie.etag, None);
    }

    #[test]
    fn a_body_that_is_not_a_multistatus_is_refused() {
        for body in ["not xml at all", "<html><body>hi</body></html>"] {
            assert!(matches!(
                parse(body, &base("/dav/")),
                Err(ParseError::Malformed)
            ));
        }
    }
}

//! Reading a Metalink document.
//!
//! Deliberately a scanner rather than an XML parser. A metalink is a small, flat document
//! whose interesting parts are three elements deep, and pulling in a full parser would mean
//! shipping an XML attack surface into a sandbox to read a list of URLs.

/// One `<file>` entry: what it is called, how big it is, where to get it and what it must
/// hash to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedFile {
    pub name: Option<String>,
    pub size: Option<u64>,
    /// The HTTP(S) mirrors, best priority first. The first is the address proposed to the
    /// LinkGrabber; the rest travel in [`ParsedFile::sources`] with everything else.
    pub urls: Vec<String>,
    /// Every mirror a runner exists for, HTTP or not, in document order with what the
    /// document said about it.
    pub sources: Vec<ParsedSource>,
    /// Whole-file digests as `(type, hex)`, exactly as the document spells them.
    pub hashes: Vec<(String, String)>,
    pub pieces: Option<ParsedPieces>,
}

/// One `<url>` of a file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedSource {
    pub url: String,
    /// Lower is preferred. Metalink 4's `priority` as written; Metalink 3's `preference`
    /// (0 to 100, higher preferred) turned around onto the same scale.
    pub priority: Option<u32>,
    pub location: Option<String>,
}

/// A `<pieces>` element: the piece length, the digest type and one hash per piece.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedPieces {
    pub algorithm: String,
    pub length: u64,
    pub hashes: Vec<String>,
}

/// Most `<url>` elements read per file. The host keeps 32; the rest is read and dropped here
/// so a hostile list cannot spend the fuel budget on addresses nobody will use.
const MAX_URLS_PER_FILE: usize = 64;
/// Most piece hashes read per file, the host's own ceiling.
const MAX_PIECE_HASHES: usize = 65_536;
/// Schemes a mirror may use. The host checks again; this keeps `magnet:` and friends out.
const SOURCE_SCHEMES: &[&str] = &["http://", "https://", "ftp://", "ftps://", "sftp://"];

/// Markers that identify a Metalink document. Version 4 uses the IETF namespace, version 3
/// the older metalinker.org one; both open a `<metalink` element.
const MARKERS: &[&str] = &[
    "urn:ietf:params:xml:ns:metalink",
    "www.metalinker.org",
    "<metalink",
];

/// Whether this text looks like a Metalink document at all.
#[must_use]
pub fn claims(input: &str) -> bool {
    let head: String = input.chars().take(4096).collect::<String>().to_lowercase();
    MARKERS.iter().any(|marker| head.contains(marker))
}

/// Every `<file>` entry the document lists, in order.
///
/// A malformed entry is skipped rather than failing the document: one broken row in a mirror
/// list should not cost somebody the rest of it.
#[must_use]
pub fn files_in(input: &str) -> Vec<ParsedFile> {
    let mut files = Vec::new();
    let mut rest = input;
    while let Some(start) = find_ignore_case(rest, "<file") {
        let after_tag = &rest[start..];
        // `<file` must be the whole element name, not the start of `<fileinfo`.
        let Some(open_end) = after_tag.find('>') else {
            break;
        };
        let open_tag = &after_tag[..=open_end];
        if !open_tag[5..].starts_with([' ', '\t', '\n', '\r', '>', '/']) {
            rest = &after_tag[open_end + 1..];
            continue;
        }
        let body_start = open_end + 1;
        let body_end = find_ignore_case(&after_tag[body_start..], "</file>")
            .map_or(after_tag.len(), |end| body_start + end);
        let body = &after_tag[body_start..body_end];
        if let Some(file) = read_file(open_tag, body) {
            files.push(file);
        }
        rest = &after_tag[body_end.min(after_tag.len())..];
        if rest.is_empty() {
            break;
        }
        // Step past the closing tag so an unterminated entry cannot loop forever.
        rest = rest.strip_prefix("</file>").unwrap_or(&rest[1..]);
    }
    files
}

fn read_file(open_tag: &str, body: &str) -> Option<ParsedFile> {
    // Piece hashes are `<hash>` elements too. Cut the `<pieces>` block out first, or every
    // piece would be read as a digest of the whole file.
    let (pieces, outside) = split_pieces(body);
    let mut sources: Vec<ParsedSource> = tagged_elements(&outside, "url", MAX_URLS_PER_FILE)
        .into_iter()
        .filter(|(_, url)| {
            SOURCE_SCHEMES
                .iter()
                .any(|scheme| starts_with_ignore_case(url, scheme))
        })
        .map(|(tag, url)| ParsedSource {
            priority: priority_of(&tag),
            location: attribute(&tag, "location").filter(|location| !location.is_empty()),
            url,
        })
        .collect();
    // Best priority first, ties in document order: the order the host will try them in.
    sources.sort_by_key(|source| source.priority.unwrap_or(u32::MAX));
    let urls: Vec<String> = sources
        .iter()
        .map(|source| source.url.clone())
        .filter(|url| {
            starts_with_ignore_case(url, "http://") || starts_with_ignore_case(url, "https://")
        })
        .collect();
    if urls.is_empty() {
        // A file with no address we can propose is not a candidate; the host would have
        // nothing to download.
        return None;
    }
    let hashes = tagged_elements(&outside, "hash", MAX_URLS_PER_FILE)
        .into_iter()
        .filter_map(|(tag, value)| {
            attribute(&tag, "type")
                .filter(|kind| !kind.is_empty())
                .map(|kind| (kind, value))
        })
        .collect();
    Some(ParsedFile {
        name: attribute(open_tag, "name").filter(|name| !name.is_empty()),
        size: elements(&outside, "size")
            .first()
            .and_then(|size| size.parse().ok()),
        urls,
        sources,
        hashes,
        pieces,
    })
}

/// Metalink 4 `priority="1"`; Metalink 3 `preference="100"`, turned into the same scale.
fn priority_of(tag: &str) -> Option<u32> {
    if let Some(priority) = attribute(tag, "priority").and_then(|value| value.trim().parse().ok()) {
        return Some(priority);
    }
    attribute(tag, "preference")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|preference| *preference <= 100)
        .map(|preference| 101 - preference)
}

/// The `<pieces>` block of a file body and the body without it.
///
/// Only the first block is read. Metalink 4 allows one per hash type; the first is enough to
/// verify, and reading further ones would only multiply the work.
fn split_pieces(body: &str) -> (Option<ParsedPieces>, String) {
    let Some(start) = find_ignore_case(body, "<pieces") else {
        return (None, body.to_owned());
    };
    let after = &body[start..];
    let Some(open_end) = after.find('>') else {
        return (None, body[..start].to_owned());
    };
    let open_tag = &after[..=open_end];
    let Some(close) = find_ignore_case(&after[open_end + 1..], "</pieces>") else {
        return (None, body[..start].to_owned());
    };
    let inner = &after[open_end + 1..open_end + 1 + close];
    let rest = &after[open_end + 1 + close + "</pieces>".len()..];
    let outside = format!("{}{}", &body[..start], rest);
    (read_pieces(open_tag, inner), outside)
}

fn read_pieces(open_tag: &str, inner: &str) -> Option<ParsedPieces> {
    let algorithm = attribute(open_tag, "type").filter(|kind| !kind.is_empty())?;
    let length = attribute(open_tag, "length")?.trim().parse().ok()?;
    // One more than the ceiling is read on purpose: a list that long is refused whole, not
    // cut to a prefix that would describe a shorter file.
    let hashes = elements_bounded(inner, "hash", MAX_PIECE_HASHES + 1);
    (hashes.len() <= MAX_PIECE_HASHES && !hashes.is_empty()).then_some(ParsedPieces {
        algorithm,
        length,
        hashes,
    })
}

/// The text content of every `<name …>…</name>` in `body`, trimmed and unescaped.
fn elements(body: &str, name: &str) -> Vec<String> {
    elements_bounded(body, name, usize::MAX)
}

/// [`elements`], stopping after `limit` matches.
fn elements_bounded(body: &str, name: &str, limit: usize) -> Vec<String> {
    tagged_elements(body, name, limit)
        .into_iter()
        .map(|(_, content)| content)
        .collect()
}

/// Every `<name …>…</name>` in `body` as its opening tag and its trimmed, unescaped content,
/// at most `limit` of them.
fn tagged_elements(body: &str, name: &str, limit: usize) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut rest = body;
    while found.len() < limit
        && let Some(start) = find_ignore_case(rest, &open)
    {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        // Guard against `<url…` matching `<urls>`: the character after the name must end it.
        if !after[open.len()..].starts_with([' ', '\t', '\n', '\r', '>', '/']) {
            rest = &after[open_end + 1..];
            continue;
        }
        let content_start = open_end + 1;
        match find_ignore_case(&after[content_start..], &close) {
            Some(end) => {
                found.push((
                    after[..=open_end].to_owned(),
                    unescape(after[content_start..content_start + end].trim()),
                ));
                rest = &after[content_start + end..];
            }
            None => break,
        }
    }
    found
}

/// The value of `name="…"` in an opening tag, single or double quoted.
fn attribute(tag: &str, name: &str) -> Option<String> {
    // ASCII only, so every offset found in `lower` is the same offset in `tag`.
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let at = from + at;
        let before_ok = at > 0 && matches!(lower.as_bytes()[at - 1], b' ' | b'\t' | b'\n' | b'\r');
        let rest = &tag[at + name.len()..];
        let value = rest.trim_start();
        if before_ok && let Some(value) = value.strip_prefix('=') {
            let value = value.trim_start();
            let quote = value.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = value[1..].find(quote)?;
                return Some(unescape(&value[1..=end]));
            }
        }
        from = at + name.len();
    }
    None
}

/// The five predefined XML entities. A metalink URL realistically carries only `&amp;`, but
/// decoding all five costs nothing and leaves no half-decoded text behind.
fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Byte offset of `needle` in `haystack`, ASCII case ignored.
///
/// Without allocating: the earlier version lowercased the whole remaining document on every
/// call, which is quadratic in a file with thousands of piece hashes. Every needle here starts
/// with `<`, so a match always begins on a character boundary.
fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

fn starts_with_ignore_case(value: &str, prefix: &str) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{ParsedFile, ParsedPieces, ParsedSource, claims, files_in};

    const META4: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="example.iso">
    <size>14471447</size>
    <hash type="sha-256">abc</hash>
    <url priority="1">https://mirror.example/example.iso</url>
    <url priority="2">https://other.example/example.iso</url>
  </file>
  <file name="notes.txt">
    <url>https://mirror.example/notes.txt</url>
  </file>
</metalink>"#;

    #[test]
    fn a_metalink_4_document_yields_its_files() {
        assert!(claims(META4));
        let files = files_in(META4);
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0],
            ParsedFile {
                name: Some("example.iso".to_owned()),
                size: Some(14_471_447),
                urls: vec![
                    "https://mirror.example/example.iso".to_owned(),
                    "https://other.example/example.iso".to_owned(),
                ],
                sources: vec![
                    ParsedSource {
                        url: "https://mirror.example/example.iso".to_owned(),
                        priority: Some(1),
                        location: None,
                    },
                    ParsedSource {
                        url: "https://other.example/example.iso".to_owned(),
                        priority: Some(2),
                        location: None,
                    },
                ],
                hashes: vec![("sha-256".to_owned(), "abc".to_owned())],
                pieces: None,
            }
        );
        assert_eq!(files[1].name.as_deref(), Some("notes.txt"));
        assert_eq!(files[1].size, None);
    }

    #[test]
    fn the_older_metalink_3_layout_is_read_the_same_way() {
        // Version 3 wraps the entries in <files> and the addresses in <resources>. Neither
        // wrapper carries anything this parser needs, so it reads through them.
        const V3: &str = r#"<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files>
    <file name="example.tar.gz">
      <size>1024</size>
      <resources>
        <url type="http">http://mirror.example/example.tar.gz</url>
      </resources>
    </file>
  </files>
</metalink>"#;
        assert!(claims(V3));
        let files = files_in(V3);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name.as_deref(), Some("example.tar.gz"));
        assert_eq!(files[0].size, Some(1024));
        assert_eq!(files[0].urls, vec!["http://mirror.example/example.tar.gz"]);
    }

    #[test]
    fn an_entry_without_a_usable_address_is_skipped() {
        // A metalink may list a torrent or an FTP mirror this parser cannot propose. Dropping
        // the entry is right; proposing an address the queue cannot fetch is not.
        const MIXED: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="only-torrent.iso"><url>magnet:?xt=urn:btih:abc</url></file>
  <file name="good.iso"><url>https://mirror.example/good.iso</url></file>
</metalink>"#;
        let files = files_in(MIXED);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name.as_deref(), Some("good.iso"));
    }

    #[test]
    fn escaped_characters_come_back_decoded() {
        const ESCAPED: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="a &amp; b.bin"><url>https://mirror.example/get?a=1&amp;b=2</url></file>
</metalink>"#;
        let files = files_in(ESCAPED);
        assert_eq!(files[0].name.as_deref(), Some("a & b.bin"));
        assert_eq!(files[0].urls, vec!["https://mirror.example/get?a=1&b=2"]);
    }

    #[test]
    fn text_that_is_not_a_metalink_is_not_claimed() {
        assert!(!claims(
            "https://example.com/one.bin\nhttps://example.com/two.bin"
        ));
        assert!(!claims("<html><body>nothing to see</body></html>"));
    }

    #[test]
    fn an_unterminated_document_stops_instead_of_looping() {
        // A truncated download is the realistic way this happens, and a scanner that keeps
        // looking for a closing tag it will never find would hang inside the fuel budget.
        let files = files_in("<metalink><file name=\"x\"><url>https://a.example/x");
        assert!(files.is_empty());
    }

    #[test]
    fn mirrors_come_back_by_priority_with_location_and_every_protocol() {
        const RANKED: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="ranked.iso">
    <size>1000</size>
    <url priority="3" location="us">https://slow.example/ranked.iso</url>
    <url priority="1" location="de">ftp://fast.example/ranked.iso</url>
    <url priority="2" location="fr">https://second.example/ranked.iso</url>
    <metaurl mediatype="torrent" priority="1">https://t.example/ranked.torrent</metaurl>
  </file>
</metalink>"#;
        let files = files_in(RANKED);
        let file = &files[0];
        let order: Vec<_> = file
            .sources
            .iter()
            .map(|source| {
                (
                    source.url.as_str(),
                    source.priority,
                    source.location.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                ("ftp://fast.example/ranked.iso", Some(1), Some("de")),
                ("https://second.example/ranked.iso", Some(2), Some("fr")),
                ("https://slow.example/ranked.iso", Some(3), Some("us")),
            ]
        );
        // The proposal is the best HTTP mirror; FTP stays a source of the set.
        assert_eq!(file.urls[0], "https://second.example/ranked.iso");
    }

    #[test]
    fn piece_hashes_are_read_apart_from_the_whole_file_hash() {
        const PIECES: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="pieces.bin">
    <size>40000</size>
    <hash type="sha-256">aaaa</hash>
    <pieces length="16384" type="sha-1">
      <hash>1111</hash>
      <hash>2222</hash>
      <hash>3333</hash>
    </pieces>
    <url>https://m.example/pieces.bin</url>
  </file>
</metalink>"#;
        let file = &files_in(PIECES)[0];
        assert_eq!(file.hashes, [("sha-256".to_owned(), "aaaa".to_owned())]);
        assert_eq!(
            file.pieces,
            Some(ParsedPieces {
                algorithm: "sha-1".to_owned(),
                length: 16_384,
                hashes: vec!["1111".to_owned(), "2222".to_owned(), "3333".to_owned()],
            })
        );
    }

    #[test]
    fn metalink_3_preference_and_verification_map_onto_the_same_fields() {
        const V3: &str = r#"<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files><file name="v3.bin">
    <size>10</size>
    <verification>
      <hash type="sha1">abcd</hash>
      <pieces length="16384" type="sha1"><hash piece="0">eeee</hash></pieces>
    </verification>
    <resources>
      <url type="http" location="uk" preference="10">http://low.example/v3.bin</url>
      <url type="http" preference="100">http://high.example/v3.bin</url>
    </resources>
  </file></files>
</metalink>"#;
        let file = &files_in(V3)[0];
        assert_eq!(file.urls[0], "http://high.example/v3.bin");
        assert_eq!(file.sources[0].priority, Some(1));
        assert_eq!(file.sources[1].priority, Some(91));
        assert_eq!(file.sources[1].location.as_deref(), Some("uk"));
        assert_eq!(file.hashes, [("sha1".to_owned(), "abcd".to_owned())]);
        assert_eq!(
            file.pieces.as_ref().map(|pieces| pieces.hashes.len()),
            Some(1)
        );
    }

    #[test]
    fn a_hostile_mirror_list_is_bounded() {
        // Thousands of mirrors and more piece hashes than the host keeps: the first is cut to
        // the per-file ceiling, the second is refused whole.
        let mut document = String::from(
            r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="big"><size>1</size>"#,
        );
        for index in 0..5_000 {
            document.push_str(&format!("<url>https://m{index}.example/big</url>"));
        }
        document.push_str(r#"<pieces length="16384" type="sha-1">"#);
        for _ in 0..=super::MAX_PIECE_HASHES {
            document.push_str("<hash>00</hash>");
        }
        document.push_str("</pieces></file></metalink>");
        let file = &files_in(&document)[0];
        assert_eq!(file.sources.len(), super::MAX_URLS_PER_FILE);
        assert!(file.pieces.is_none());
    }

    #[test]
    fn traversal_in_a_file_name_is_left_for_the_host_to_sanitise() {
        // The parser reports the name as written; `rd_files::sanitize_file_name` is the one
        // place a name is made safe, and a second, different rule here would only disagree.
        const TRAVERSAL: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="../../etc/passwd"><url>https://m.example/x</url></file>
</metalink>"#;
        assert_eq!(
            files_in(TRAVERSAL)[0].name.as_deref(),
            Some("../../etc/passwd")
        );
    }

    #[test]
    fn mixed_case_markup_and_non_ascii_text_keep_their_offsets() {
        const MIXED: &str = r#"<Metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <FILE NAME="Çalış İst.bin"><URL Priority="1" LOCATION="at">https://m.example/g</URL></FILE>
</Metalink>"#;
        let file = &files_in(MIXED)[0];
        assert_eq!(file.name.as_deref(), Some("Çalış İst.bin"));
        assert_eq!(file.sources[0].location.as_deref(), Some("at"));
        assert_eq!(file.sources[0].priority, Some(1));
    }
}

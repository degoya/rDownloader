//! Reading a Metalink document.
//!
//! Deliberately a scanner rather than an XML parser. A metalink is a small, flat document
//! whose interesting parts are three elements deep, and pulling in a full parser would mean
//! shipping an XML attack surface into a sandbox to read a list of URLs.

use plugin_common::html::decode_entities;

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

/// The code a claimed document is refused under when no `<file>` in it carries a usable
/// address (RD-191-07, PLUG-16). The catalogues in `locales/` carry it; it used to be declared
/// there and never sent, so such a document was answered with silence.
pub const UNREADABLE: &str = "metalink_intake.unreadable";

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
                    decode_entities(after[content_start..content_start + end].trim()),
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
                return Some(decode_entities(&value[1..=end]));
            }
        }
        from = at + name.len();
    }
    None
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
#[path = "parse_tests.rs"]
mod tests;

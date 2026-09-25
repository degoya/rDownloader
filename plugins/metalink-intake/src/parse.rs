//! Reading a Metalink document.
//!
//! Deliberately a scanner rather than an XML parser. A metalink is a small, flat document
//! whose interesting parts are three elements deep, and pulling in a full parser would mean
//! shipping an XML attack surface into a sandbox to read a list of URLs.

/// One `<file>` entry: what it is called, how big it is, and where to get it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedFile {
    pub name: Option<String>,
    pub size: Option<u64>,
    /// Mirrors in document order. Metalink lists several; the host takes the first it can use.
    pub urls: Vec<String>,
}

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
    let urls: Vec<String> = elements(body, "url")
        .into_iter()
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        .collect();
    if urls.is_empty() {
        // A file with no address we can fetch is not a candidate; the host would have
        // nothing to download.
        return None;
    }
    Some(ParsedFile {
        name: attribute(open_tag, "name").filter(|name| !name.is_empty()),
        size: elements(body, "size")
            .first()
            .and_then(|size| size.parse().ok()),
        urls,
    })
}

/// The text content of every `<name …>…</name>` in `body`, trimmed and unescaped.
fn elements(body: &str, name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut rest = body;
    while let Some(start) = find_ignore_case(rest, &open) {
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
                found.push(unescape(after[content_start..content_start + end].trim()));
                rest = &after[content_start + end..];
            }
            None => break,
        }
    }
    found
}

/// The value of `name="…"` in an opening tag, single or double quoted.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
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

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_lowercase().find(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{ParsedFile, claims, files_in};

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
}

//! Reading the links out of a listing page (RD-110-21).
//!
//! A regular expression over the markup rather than a parser, and that is a considered
//! choice, not a shortcut. The whole question here is "which anchors does this page carry",
//! the answer is filtered immediately afterwards by the rules' own `match` — an address no
//! rule claims is discarded — and a release page's own rule then re-reads the page properly.
//! Adding an HTML parser to this crate would buy correctness on markup that never reaches a
//! decision.
//!
//! Anchor *text* is treated as a name, so it is stripped of tags, has its entities decoded
//! and its whitespace collapsed. Nothing else is interpreted.

use std::sync::LazyLock;

use regex::Regex;

/// `<a … href="…">…</a>`, with the two quoting styles a page may use.
static ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\s[^>]*?href\s*=\s*(?:"([^"]*)"|'([^']*)')[^>]*>(.*?)</a\s*>"#)
        .expect("the anchor pattern is a constant")
});

/// Any tag, for taking the markup out of anchor text.
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("the tag pattern is a constant"));

/// Every anchor of a document as `(href, text)`, in the order the page carries them.
#[must_use]
pub fn anchors(body: &str) -> Vec<(String, String)> {
    ANCHOR
        .captures_iter(body)
        .filter_map(|captures| {
            let href = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|found| found.as_str())?;
            let href = decode(href.trim());
            if href.is_empty() {
                return None;
            }
            let text = captures.get(3).map_or("", |found| found.as_str());
            Some((href, text_of(text)))
        })
        .collect()
}

/// Anchor text as a name: no tags, entities decoded, whitespace collapsed.
fn text_of(inner: &str) -> String {
    let without_tags = TAG.replace_all(inner, " ");
    decode(&without_tags)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The named entities and the numeric forms that occur in a title or an address.
///
/// Deliberately not a complete entity table: anything else stays as it is, which costs a
/// name one odd character and never turns one address into another. An `&` that begins no
/// entity — `/a&b/`, `R &amp D` — is left exactly where it is for the same reason.
fn decode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let name: String = rest[1..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '#')
            .take(MAX_ENTITY)
            .collect();
        let terminated = rest[1 + name.len()..].starts_with(';');
        match terminated
            .then(|| named(&name).or_else(|| numeric(&name)))
            .flatten()
        {
            Some(character) => {
                out.push(character);
                rest = &rest[name.len() + 2..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Longest entity name read. `&thetasym;` is nine characters and nothing here needs it.
const MAX_ENTITY: usize = 10;

fn named(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => None,
    }
}

fn numeric(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::anchors;

    #[test]
    fn every_anchor_comes_back_with_its_text() {
        let page = r#"<ul>
            <li><a class="thumb" href="/movies/one/"><img src="x.jpg"></a></li>
            <li><a href='/movies/one/'>Show.S01E01.1080p</a></li>
            <li><a href="https://other.test/x?a=1&amp;b=2" rel="nofollow">Other &amp; More</a></li>
        </ul>"#;
        let found = anchors(page);
        assert_eq!(
            found,
            vec![
                ("/movies/one/".to_owned(), String::new()),
                ("/movies/one/".to_owned(), "Show.S01E01.1080p".to_owned()),
                (
                    "https://other.test/x?a=1&b=2".to_owned(),
                    "Other & More".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn markup_inside_the_text_is_taken_out_rather_than_kept() {
        let page = "<a href=\"/a/\"><span>Show</span>\n <b>S01E02</b> 1080p</a>";
        assert_eq!(
            anchors(page),
            vec![("/a/".to_owned(), "Show S01E02 1080p".to_owned())]
        );
    }

    #[test]
    fn an_anchor_without_an_address_is_not_a_link() {
        assert!(anchors("<a name=\"top\">Top</a><a href=\"\">Empty</a>").is_empty());
    }

    #[test]
    fn a_stray_ampersand_survives_unchanged() {
        // Half a decoder is worse than none: an address must never be altered by a guess.
        let page = "<a href=\"/a&b/\">R &amp D &#38; more &#x26; on</a>";
        assert_eq!(
            anchors(page),
            vec![("/a&b/".to_owned(), "R &amp D & more & on".to_owned())]
        );
    }
}

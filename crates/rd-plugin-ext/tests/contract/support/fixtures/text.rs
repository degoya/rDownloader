//! The fixtures read as text rather than JSON: an HTML or WebDAV listing, a raw capture. What
//! they may name is checked by scanning, with the scanners the Nextcloud and directory-index
//! modules each had a copy of.

use std::path::Path;

/// The two share tokens the Nextcloud fixtures are allowed to name. Both invented.
pub const SHARE_TOKENS: &[&str] = &["QxT7bK2mNp9wZr4", "mK4pQ8vR2tY6nL0"];
/// The request tokens the two Nextcloud pages carry. Both invented.
pub const REQUEST_TOKENS: &[&str] = &["sBQ1cUZbW0E9V2xsVGZlQg==", "Rk9PYkFyMTIzNDU2Nzg5MA=="];

/// What a text fixture may not carry, and the only things it may name.
pub struct Text {
    /// Words no document may contain at all.
    pub absent: &'static [&'static str],
    /// The hosts a document may name, by suffix; empty with `hosts`: hosts are not checked.
    pub host_suffixes: &'static [&'static str],
    /// Hosts a document may name exactly, beside the suffixes.
    pub hosts: &'static [&'static str],
    /// `(marker, the only values allowed after it)`.
    pub tokens: &'static [(&'static str, &'static [&'static str])],
    /// `(attribute opening, the only values allowed in it)`.
    pub attributes: &'static [(&'static str, &'static [&'static str])],
    /// Words after which no encoded value may follow.
    pub encoded_after: &'static [&'static str],
}

impl Text {
    /// A capture that may not contain these words, and is not checked otherwise.
    pub const fn absent(words: &'static [&'static str]) -> Self {
        Self {
            absent: words,
            host_suffixes: &[],
            hosts: &[],
            tokens: &[],
            attributes: &[],
            encoded_after: &[],
        }
    }

    pub fn check(&self, path: &Path, body: &str) {
        for word in self.absent {
            assert!(
                !body.contains(*word),
                "{path:?} carries `{word}`; a credential belongs in the test, not here"
            );
        }
        if !self.host_suffixes.is_empty() || !self.hosts.is_empty() {
            for authority in authorities(body) {
                assert!(
                    self.host_suffixes
                        .iter()
                        .any(|suffix| authority.ends_with(*suffix))
                        || self.hosts.contains(&authority.as_str()),
                    "{path:?} names the host {authority}, which is not a documentation domain"
                );
            }
        }
        for (marker, allowed) in self.tokens {
            for token in values_after(body, marker) {
                assert!(
                    allowed.contains(&token.as_str()),
                    "{path:?} carries a token after `{marker}` that is not a placeholder: {token}"
                );
            }
        }
        for (marker, allowed) in self.attributes {
            for token in attribute_values(body, marker) {
                assert!(
                    allowed.contains(&token.as_str()),
                    "{path:?} carries a `{marker}` value that is not a placeholder: {token}"
                );
            }
        }
        // sabre/dav's own 401 message names the `Authorization: Basic` header, so the word alone
        // proves nothing. What must not be here is an encoded *value* after it.
        for marker in self.encoded_after {
            for value in tokens_after(body, marker) {
                assert!(
                    !credential_like(&value),
                    "{path:?} carries an encoded credential after `{marker}`: {value}"
                );
            }
        }
    }
}

/// Every value following `marker`, up to the first character a token cannot contain.
fn values_after(text: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let tail = &rest[index + marker.len()..];
        let end = tail
            .find(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_')
            })
            .unwrap_or(tail.len());
        if end > 0 {
            out.push(tail[..end].to_owned());
        }
        rest = &tail[end..];
    }
    out
}

/// Every attribute value opened by `marker` and closed by the next double quote.
fn attribute_values(text: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let tail = &rest[index + marker.len()..];
        let Some(end) = tail.find('"') else { break };
        out.push(tail[..end].to_owned());
        rest = &tail[end + 1..];
    }
    out
}

/// Every `://<authority>` in `text`, lowercased.
fn authorities(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        let tail = &rest[index + 3..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '"' | '\'' | '<' | '>')
            })
            .unwrap_or(tail.len());
        out.push(tail[..end].to_ascii_lowercase());
        rest = &tail[end..];
    }
    out
}

/// Every whitespace-delimited token following `marker`.
fn tokens_after(text: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let tail = &rest[index + marker.len()..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>')
            })
            .unwrap_or(tail.len());
        out.push(tail[..end].to_owned());
        rest = &tail[end..];
    }
    out
}

/// Whether a token is an encoded value rather than a word: base64 and its relatives always
/// carry something that is not a letter, and a word never carries only those.
fn credential_like(value: &str) -> bool {
    value.len() >= 8
        && value
            .chars()
            .any(|character| character.is_ascii_digit() || matches!(character, '+' | '/' | '='))
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=' | '_' | '-')
        })
}

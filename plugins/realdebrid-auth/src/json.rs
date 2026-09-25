//! Reading five fields out of a JSON document, without a parser.
//!
//! The same reader the `oauth` SDK template carries (`sdk/templates/oauth/src/pkce.rs`), kept
//! here for the same reason it exists there: a guest has no WASI and every dependency is
//! bytes in a signed package, so pulling a full JSON parser in to read `access_token` and four
//! numbers would be paying for a library with the plugin's whole size budget.
//!
//! It is deliberately not a JSON parser. It finds `"name":` and reads the value after it, which
//! is right for the flat answers an OAuth endpoint gives and would be wrong for a nested
//! document. The plugin never asks it for one.

/// The string value of a JSON field.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
    let mut rest = value_after(body, name)?.strip_prefix('"')?;
    let mut out = String::new();
    loop {
        let mut chars = rest.chars();
        let character = chars.next()?;
        rest = chars.as_str();
        match character {
            '"' => return Some(out),
            '\\' => {
                let mut escaped = rest.chars();
                match escaped.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
                rest = escaped.as_str();
            }
            other => out.push(other),
        }
    }
}

/// The numeric value of a JSON field. Negative numbers are `None`: every number this plugin
/// reads is a count of seconds or a documented error code, and none of them is ever negative.
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
    let rest = value_after(body, name)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn value_after<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let after = &body[from + at + needle.len()..];
        if let Some(value) = after.trim_start().strip_prefix(':') {
            return Some(value.trim_start());
        }
        from += at + needle.len();
    }
    None
}

/// Percent-encoding for the query parameters the device endpoint takes.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_answer_reads_field_by_field() {
        let body = r#"{"access_token":"AT","expires_in":3600,"token_type":"Bearer"}"#;
        assert_eq!(string_field(body, "access_token").as_deref(), Some("AT"));
        assert_eq!(number_field(body, "expires_in"), Some(3600));
        assert_eq!(string_field(body, "refresh_token"), None);
    }

    #[test]
    fn an_escaped_address_comes_back_unescaped() {
        let body = r#"{"verification_url":"https:\/\/real-debrid.com\/device"}"#;
        assert_eq!(
            string_field(body, "verification_url").as_deref(),
            Some("https://real-debrid.com/device")
        );
    }

    #[test]
    fn a_negative_or_absent_number_is_no_number() {
        assert_eq!(number_field(r#"{"interval":-1}"#, "interval"), None);
        assert_eq!(number_field("{}", "interval"), None);
    }

    #[test]
    fn percent_encoding_leaves_the_unreserved_set_alone() {
        assert_eq!(percent_encode("X245A4XAIBGVM"), "X245A4XAIBGVM");
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
    }
}

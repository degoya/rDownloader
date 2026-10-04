//! Percent-encoding, once (RD-191-07, PLUG-12).
//!
//! Eight plugins carried a copy of the same twelve lines — every one of them RFC 3986's
//! unreserved set and nothing else, which is the only rule that reads the same in a query, a
//! form body and a path segment. Plain Rust with no dependencies, so a guest that takes it
//! gains no import.

use std::fmt::Write;

/// Percent-encodes everything outside RFC 3986's unreserved set, so a value cannot end the
/// query, the form field or the path segment it sits in — a torrent name containing `&tr=`
/// would otherwise add a tracker of its choosing.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            // Writing into a String cannot fail; the result is discarded rather than unwrapped.
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// Decodes `%XX` escapes into bytes; a `%` that is not followed by two hex digits is kept as
/// it stands. `+` is left alone: it means a space only in a form body, never in a header.
#[must_use]
pub fn percent_decode(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(at + 1).and_then(|byte| hex_value(*byte)),
                bytes.get(at + 2).and_then(|byte| hex_value(*byte)),
            )
        {
            out.push((high << 4) | low);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    out
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{percent_decode, percent_encode};

    #[test]
    fn a_value_cannot_break_out_of_the_query_it_sits_in() {
        assert_eq!(percent_encode("a&b=c d"), "a%26b%3Dc%20d");
        assert_eq!(percent_encode("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(percent_encode("/+?#"), "%2F%2B%3F%23");
        assert_eq!(percent_encode("\u{fc}"), "%C3%BC");
    }

    #[test]
    fn decoding_reverses_encoding_and_keeps_a_stray_percent() {
        assert_eq!(percent_decode("a%26b%3Dc%20d"), b"a&b=c d");
        assert_eq!(percent_decode("%C3%BC"), "\u{fc}".as_bytes());
        assert_eq!(percent_decode("100%"), b"100%");
        assert_eq!(percent_decode("%zz%4"), b"%zz%4");
        assert_eq!(percent_decode("a+b"), b"a+b");
    }
}

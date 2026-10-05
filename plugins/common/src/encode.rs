//! Percent-encoding and hex, once (RD-191-07, PLUG-12; RD-1110-04).
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

/// [`percent_decode`] read as text, a sequence that is not UTF-8 replaced rather than refused:
/// for a name that is shown, where a stray byte is no reason to show nothing.
#[must_use]
pub fn percent_decode_lossy(value: &str) -> String {
    String::from_utf8_lossy(&percent_decode(value)).into_owned()
}

/// Decodes `%XX` escapes into text, or `None` when a `%` is not followed by two hex digits or
/// the result is not UTF-8: for a name read out of an address, where a broken escape means the
/// address is not one. `+` is left alone, as in [`percent_decode`].
#[must_use]
pub fn percent_decode_strict(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let high = hex_value(*bytes.get(at + 1)?)?;
            let low = hex_value(*bytes.get(at + 2)?)?;
            out.push((high << 4) | low);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Lower-case hex of `bytes`, two digits each: how a digest is written down and compared.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing into a String cannot fail; the result is discarded rather than unwrapped.
        let _ = write!(out, "{byte:02x}");
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
    use super::{
        percent_decode, percent_decode_lossy, percent_decode_strict, percent_encode, to_hex,
    };

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

    #[test]
    fn lossy_decoding_replaces_what_is_not_utf8() {
        assert_eq!(percent_decode_lossy("a%20b%"), "a b%");
        assert_eq!(percent_decode_lossy("%FFx"), "\u{fffd}x");
    }

    #[test]
    fn strict_decoding_refuses_a_broken_escape_and_invalid_text() {
        assert_eq!(percent_decode_strict("a%26b%2fc"), Some("a&b/c".to_owned()));
        assert_eq!(percent_decode_strict("a+b"), Some("a+b".to_owned()));
        assert_eq!(percent_decode_strict("a%2"), None);
        assert_eq!(percent_decode_strict("%zz"), None);
        assert_eq!(percent_decode_strict("%FF"), None);
    }

    #[test]
    fn hex_is_lower_case_and_zero_padded() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(to_hex(&[]), "");
    }
}

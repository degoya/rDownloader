//! `decode`: the five ways a page hides an address, and no sixth.
//!
//! **There is no JavaScript interpreter here, and there will not be one.** JDownloader's
//! `GenericBase64Decrypter` covers six services on its own, and between base64, hex, rot13,
//! percent-encoding and concatenated string literals almost every obfuscation these pages
//! use is arithmetic on text, not a program. What is genuinely a program stays undecodable,
//! and the run says so with `site_rules.decode_failed` — an honest refusal, not an HTML file
//! in the queue (RD-110-07). Running a stranger's script to find a download link would hand
//! a release page the same power a plugin has, without the sandbox a plugin runs in.

use base64::Engine as _;

use crate::step::Decoding;

/// Decodes one string, or says nothing came out.
pub(crate) fn decode(encoding: Decoding, input: &str) -> Option<String> {
    match encoding {
        Decoding::Base64 => base64(input),
        Decoding::Hex => hex(input),
        Decoding::Rot13 => Some(rot13(input)),
        Decoding::Url => percent_encoding::percent_decode_str(input)
            .decode_utf8()
            .ok()
            .map(std::borrow::Cow::into_owned),
        Decoding::JsString => js_string(input),
    }
}

/// The name as the format spells it, for the refusal.
pub(crate) fn name(encoding: Decoding) -> &'static str {
    match encoding {
        Decoding::Base64 => "base64",
        Decoding::Hex => "hex",
        Decoding::Rot13 => "rot13",
        Decoding::Url => "url",
        Decoding::JsString => "js-string",
    }
}

/// Standard and URL-safe alphabets, whitespace ignored, padding optional: a page writes
/// base64 in whichever of the four ways its framework happened to.
fn base64(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| match character {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .filter(|character| *character != '=')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(cleaned)
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn hex(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let bytes = hex::decode(cleaned).ok()?;
    String::from_utf8(bytes).ok()
}

fn rot13(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            'a'..='z' => rotate(character, b'a'),
            'A'..='Z' => rotate(character, b'A'),
            other => other,
        })
        .collect()
}

fn rotate(character: char, base: u8) -> char {
    let offset = (character as u8) - base;
    char::from(base + (offset + 13) % 26)
}

/// Concatenated JavaScript string literals: `"aHR0" + 'cHM6'`. Every literal in the input is
/// unescaped and the pieces are joined in order; whatever sits between them — `+`, a name, a
/// call — is ignored, because it is arithmetic on text and not a program.
fn js_string(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut characters = input.chars().peekable();
    let mut found = false;
    while let Some(character) = characters.next() {
        let quote = match character {
            '"' | '\'' | '`' => character,
            _ => continue,
        };
        found = true;
        while let Some(inner) = characters.next() {
            if inner == quote {
                break;
            }
            if inner != '\\' {
                out.push(inner);
                continue;
            }
            match characters.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('v') => out.push('\u{b}'),
                Some('0') => out.push('\0'),
                Some('x') => out.push(unicode_escape(&mut characters, 2)?),
                Some('u') => out.push(braced_or_fixed(&mut characters)?),
                Some(other) => out.push(other),
                None => return None,
            }
        }
    }
    found.then_some(out)
}

fn braced_or_fixed(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    if characters.peek() != Some(&'{') {
        return unicode_escape(characters, 4);
    }
    characters.next();
    let mut digits = String::new();
    for character in characters.by_ref() {
        if character == '}' {
            break;
        }
        digits.push(character);
    }
    char::from_u32(u32::from_str_radix(&digits, 16).ok()?)
}

fn unicode_escape(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    width: usize,
) -> Option<char> {
    let digits: String = characters.by_ref().take(width).collect();
    if digits.len() != width {
        return None;
    }
    char::from_u32(u32::from_str_radix(&digits, 16).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_reads_both_alphabets_with_and_without_padding() {
        let expected = Some("https://a.test/x?y=1".to_owned());
        assert_eq!(
            decode(Decoding::Base64, "aHR0cHM6Ly9hLnRlc3QveD95PTE="),
            expected
        );
        assert_eq!(
            decode(Decoding::Base64, "aHR0cHM6Ly9hLnRlc3QveD95PTE"),
            expected
        );
        assert_eq!(
            decode(Decoding::Base64, "aHR0cHM6Ly9h\nLnRlc3QveD95PTE"),
            expected
        );
        assert_eq!(decode(Decoding::Base64, "++not base64++"), None);
        assert_eq!(decode(Decoding::Base64, ""), None);
    }

    #[test]
    fn the_url_safe_alphabet_reads_too() {
        assert_eq!(
            decode(Decoding::Base64, "aHR0cHM6Ly9hLnRlc3QvP2E9Yj4-"),
            Some("https://a.test/?a=b>>".to_owned())
        );
    }

    #[test]
    fn hex_and_rot13_and_percent_encoding_decode() {
        assert_eq!(
            decode(Decoding::Hex, "68747470733a2f2f612e74657374"),
            Some("https://a.test".to_owned())
        );
        assert_eq!(decode(Decoding::Hex, "zz"), None);
        assert_eq!(
            decode(Decoding::Rot13, "uggcf://n.grfg"),
            Some("https://a.test".to_owned())
        );
        assert_eq!(
            decode(Decoding::Url, "https%3A%2F%2Fa.test%2Fa%20b"),
            Some("https://a.test/a b".to_owned())
        );
    }

    #[test]
    fn concatenated_literals_join_in_order_and_unescape() {
        assert_eq!(
            decode(Decoding::JsString, r#"var u = "https:" + '//a' + `.test`;"#),
            Some("https://a.test".to_owned())
        );
        assert_eq!(
            decode(Decoding::JsString, r#""a\x2fb/c\u{2f}d\\e""#),
            Some("a/b/c/d\\e".to_owned())
        );
        // Nothing that looks like a literal is nothing to decode, not an empty answer.
        assert_eq!(decode(Decoding::JsString, "atob(x)"), None);
    }

    #[test]
    fn every_encoding_has_a_name_for_the_refusal() {
        for (encoding, expected) in [
            (Decoding::Base64, "base64"),
            (Decoding::Hex, "hex"),
            (Decoding::Rot13, "rot13"),
            (Decoding::Url, "url"),
            (Decoding::JsString, "js-string"),
        ] {
            assert_eq!(name(encoding), expected);
        }
    }
}

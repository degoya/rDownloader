//! What a person handed over, and the key that stands for it.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point —
//! the content key is what keeps a provider whose `addMagnet` is not idempotent from being
//! asked twice, and a key that needed a request could not be written down before the first
//! one. See `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! For both shapes the key is the BitTorrent info hash, lower-case hex. A magnet carries it
//! (`xt=urn:btih:`), in hex or in base32; a `.torrent` file is the SHA-1 of the bencoded value
//! of its top-level `info` key, which is the same number arrived at the long way. Being the
//! same number for both shapes is what lets a magnet and the matching file find the same
//! remote job — and what lets `adopt` recognise it in Real-Debrid's own `hash` field.

use sha1::{Digest, Sha1};

/// Longest container this plugin will read. A `.torrent` is kilobytes; anything far past that
/// is not one, and scanning it would spend the invocation's whole budget finding that out.
pub const MAX_CONTAINER_BYTES: usize = 4 * 1024 * 1024;

/// Deepest nesting a bencoded container may have before it is refused.
///
/// A container comes from a stranger. Without this, `llllll…` is a stack overflow written in
/// eleven characters.
const MAX_BENCODE_DEPTH: u32 = 32;

/// The info hash a magnet address names, as lower-case hex.
///
/// `None` for anything that is not a magnet, and for a magnet whose `xt` is not a BitTorrent
/// info hash — `urn:sha1:` and `urn:ed2k:` are legal in a magnet and mean something else.
#[must_use]
pub fn magnet_info_hash(address: &str) -> Option<String> {
    let rest = address
        .strip_prefix("magnet:?")
        .or_else(|| address.strip_prefix("MAGNET:?"))?;
    for pair in rest.split('&') {
        let (name, value) = pair.split_once('=')?;
        // `xt.1` and friends: a magnet may name several topics, and each is a candidate.
        if name != "xt" && !name.starts_with("xt.") {
            continue;
        }
        let Some(raw) = value
            .strip_prefix("urn:btih:")
            .or_else(|| value.strip_prefix("urn%3Abtih%3A"))
        else {
            continue;
        };
        if let Some(hash) = normalise_info_hash(raw) {
            return Some(hash);
        }
    }
    None
}

/// Accepts a 40-character hex or a 32-character base32 info hash and answers lower-case hex.
///
/// Both spellings are in the field and both mean the same twenty bytes, so normalising here
/// means a magnet copied from two different sites produces one remote job and not two.
#[must_use]
pub fn normalise_info_hash(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() == 40 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(raw.to_ascii_lowercase());
    }
    if raw.len() == 32 {
        return decode_base32(raw).map(|bytes| to_hex(&bytes));
    }
    None
}

/// The info hash of a bencoded container: SHA-1 over the *encoded* value of its `info` key.
///
/// The bytes are hashed exactly as they lay in the file rather than re-encoded, because a
/// re-encoding that ordered one key differently would produce a different hash for the same
/// torrent — and the whole value of the key is that it is the same number everybody else has.
#[must_use]
pub fn container_info_hash(bytes: &[u8]) -> Option<String> {
    let info = info_slice(bytes)?;
    let mut hasher = Sha1::new();
    hasher.update(info);
    Some(to_hex(&hasher.finalize()))
}

/// The encoded bytes of the top-level `info` value.
fn info_slice(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() > MAX_CONTAINER_BYTES || bytes.first() != Some(&b'd') {
        return None;
    }
    let mut at = 1;
    while at < bytes.len() && bytes[at] != b'e' {
        let (key, after_key) = read_byte_string(bytes, at)?;
        let end = skip_value(bytes, after_key, 0)?;
        if key == b"info" {
            return bytes.get(after_key..end);
        }
        at = end;
    }
    None
}

/// Reads `<length>:<bytes>` at `at`, answering the bytes and the offset just past them.
fn read_byte_string(bytes: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let colon = bytes.iter().skip(at).position(|byte| *byte == b':')? + at;
    let digits = bytes.get(at..colon)?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // A length is bounded by the container, so a header claiming more than it holds is
    // refused rather than trusted into an overflow.
    let length: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
    let start = colon.checked_add(1)?;
    let end = start.checked_add(length)?;
    Some((bytes.get(start..end)?, end))
}

/// The offset just past the bencoded value starting at `at`.
fn skip_value(bytes: &[u8], at: usize, depth: u32) -> Option<usize> {
    if depth > MAX_BENCODE_DEPTH {
        return None;
    }
    match bytes.get(at)? {
        b'i' => {
            let end = bytes.iter().skip(at).position(|byte| *byte == b'e')? + at;
            Some(end + 1)
        }
        b'l' => {
            let mut at = at + 1;
            while *bytes.get(at)? != b'e' {
                at = skip_value(bytes, at, depth + 1)?;
            }
            Some(at + 1)
        }
        b'd' => {
            let mut at = at + 1;
            while *bytes.get(at)? != b'e' {
                let (_, after_key) = read_byte_string(bytes, at)?;
                at = skip_value(bytes, after_key, depth + 1)?;
            }
            Some(at + 1)
        }
        byte if byte.is_ascii_digit() => read_byte_string(bytes, at).map(|(_, end)| end),
        _ => None,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write;
        // Writing into a String cannot fail; the result is discarded rather than unwrapped.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// RFC 4648 base32 without padding, as a magnet spells an info hash.
fn decode_base32(text: &str) -> Option<Vec<u8>> {
    let mut accumulator: u64 = 0;
    let mut bits = 0_u32;
    let mut bytes = Vec::with_capacity(20);
    for character in text.chars() {
        let value = match character.to_ascii_uppercase() {
            letter @ 'A'..='Z' => u64::from(letter as u8 - b'A'),
            digit @ '2'..='7' => u64::from(digit as u8 - b'2') + 26,
            _ => return None,
        };
        accumulator = (accumulator << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            let byte = u8::try_from((accumulator >> bits) & 0xff).ok()?;
            bytes.push(byte);
            // The consumed bits are dropped rather than left in the accumulator. Without
            // this it grows by five bits per character and overflows on the thirteenth,
            // which in a debug build is a panic and in a release build a wrong answer.
            accumulator &= (1_u64 << bits) - 1;
        }
    }
    (bytes.len() == 20).then_some(bytes)
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

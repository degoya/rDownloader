//! Just enough bencode to find a torrent's `info` dictionary and read a few strings out of it.
//!
//! A container comes from a stranger, so every reader here is bounded: by the container's size,
//! by nesting depth, and by lengths that must fit inside the bytes they claim. Nothing is
//! decoded into a tree — a value is a byte range of the original, which is what lets the
//! `info` dictionary be hashed exactly as it lay in the file.

use std::ops::Range;

/// Largest container read at all. A `.torrent` is usually kilobytes; one for a very large
/// release with small pieces reaches megabytes, and anything far past that is not one —
/// scanning it would spend the invocation's whole budget finding that out. TorBox's plugin
/// allowed eight MiB and the others four; the larger bound is the one kept, so no plugin that
/// reads a torrent through this crate refuses one another of them accepts. `premiumize-transfers`
/// is the exception: it reads containers with `premiumize-common`, which stays at four MiB
/// (see `premiumize_common::container::MAX_CONTAINER_BYTES`).
pub const MAX_CONTAINER_BYTES: usize = 8 * 1024 * 1024;

/// Deepest nesting a value may have before it is refused. Without this, `llllll…` is a stack
/// overflow written in eleven characters.
const MAX_DEPTH: u32 = 32;

/// One bencoded dictionary, as a map from key to the byte range of its value.
pub struct Dictionary {
    entries: Vec<(Vec<u8>, Range<usize>)>,
}

impl Dictionary {
    /// Reads the dictionary starting at `at`, or `None` when there is not one there.
    #[must_use]
    pub fn at(bytes: &[u8], at: usize) -> Option<Self> {
        if bytes.len() > MAX_CONTAINER_BYTES || bytes.get(at) != Some(&b'd') {
            return None;
        }
        let mut entries = Vec::new();
        let mut cursor = at + 1;
        while *bytes.get(cursor)? != b'e' {
            let (key, after_key) = byte_string(bytes, cursor)?;
            let end = value_end(bytes, after_key)?;
            entries.push((key.to_vec(), after_key..end));
            cursor = end;
        }
        Some(Self { entries })
    }

    /// The byte range of the value stored under `key`.
    #[must_use]
    pub fn value(&self, key: &[u8]) -> Option<Range<usize>> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, range)| range.clone())
    }
}

/// The encoded bytes of a container's top-level `info` value.
#[must_use]
pub fn info_slice(bytes: &[u8]) -> Option<&[u8]> {
    let root = Dictionary::at(bytes, 0)?;
    bytes.get(root.value(b"info")?)
}

/// The byte ranges of the items of the bencoded list at `at`; empty when there is none.
#[must_use]
pub fn list_items(bytes: &[u8], at: usize) -> Vec<Range<usize>> {
    let mut items = Vec::new();
    if bytes.get(at) != Some(&b'l') {
        return items;
    }
    let mut cursor = at + 1;
    while bytes.get(cursor).is_some_and(|byte| *byte != b'e') {
        let Some(end) = value_end(bytes, cursor) else {
            break;
        };
        items.push(cursor..end);
        cursor = end;
    }
    items
}

/// The text of the byte string at the start of `range`, when it is one and it is UTF-8.
#[must_use]
pub fn text_of(bytes: &[u8], range: Range<usize>) -> Option<String> {
    let (value, _) = byte_string(bytes, range.start)?;
    std::str::from_utf8(value).ok().map(str::to_owned)
}

/// Reads `<length>:<bytes>` at `at`, answering the bytes and the offset just past them.
#[must_use]
pub fn byte_string(bytes: &[u8], at: usize) -> Option<(&[u8], usize)> {
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
#[must_use]
pub fn value_end(bytes: &[u8], at: usize) -> Option<usize> {
    skip(bytes, at, 0)
}

fn skip(bytes: &[u8], at: usize, depth: u32) -> Option<usize> {
    if depth > MAX_DEPTH {
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
                at = skip(bytes, at, depth + 1)?;
            }
            Some(at + 1)
        }
        b'd' => {
            let mut at = at + 1;
            while *bytes.get(at)? != b'e' {
                let (_, after_key) = byte_string(bytes, at)?;
                at = skip(bytes, after_key, depth + 1)?;
            }
            Some(at + 1)
        }
        byte if byte.is_ascii_digit() => byte_string(bytes, at).map(|(_, end)| end),
        _ => None,
    }
}

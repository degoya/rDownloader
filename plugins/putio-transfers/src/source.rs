//! What a person handed over, the key that stands for it, and the one address Put.io takes.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point —
//! the content key is what keeps a provider whose `transfers/add` is not idempotent from being
//! asked twice, and a key that needed a request could not be written down before the first
//! one. See `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! For both shapes the key is the BitTorrent info hash, lower-case hex. A magnet carries it
//! (`xt=urn:btih:`), in hex or in base32; a `.torrent` file is the SHA-1 of the bencoded value
//! of its top-level `info` key, which is the same number arrived at the long way. Being the
//! same number for both shapes is what lets a magnet and the matching file find the same
//! remote job — and what lets `adopt` recognise it in what Put.io says about a transfer.
//!
//! **A container is submitted as a magnet.** `POST /v2/transfers/add` takes one `url` field
//! and Put.io's only way to accept the bytes of a `.torrent` is a separate resumable upload
//! protocol on a separate host — several requests, a session to keep and a second domain to
//! grant, inside a world whose calls are all meant to be short. So [`container_magnet`] reads
//! the container once, locally, and writes the magnet it is equivalent to: the same info hash,
//! the torrent's own name, and every tracker the file listed. What that costs is a torrent
//! carrying neither trackers nor peers reachable over DHT, which Put.io then cannot start; what
//! it buys is one host, one request, and no upload session that a restart could lose.

use sha1::{Digest, Sha1};

/// Longest container this plugin will read. A `.torrent` is kilobytes; anything far past that
/// is not one, and scanning it would spend the invocation's whole budget finding that out.
pub const MAX_CONTAINER_BYTES: usize = 4 * 1024 * 1024;

/// Deepest nesting a bencoded container may have before it is refused.
///
/// A container comes from a stranger. Without this, `llllll…` is a stack overflow written in
/// eleven characters.
const MAX_BENCODE_DEPTH: u32 = 32;

/// Most trackers carried over into a reconstructed magnet. A tracker list is attacker-supplied
/// and an address has to fit in a request line; thirty is more than any real torrent lists and
/// well inside what Put.io will read.
const MAX_TRACKERS: usize = 30;

/// Longest torrent name carried over as `dn`. It is a display name and nothing depends on it,
/// so it is bounded rather than trusted.
const MAX_NAME_BYTES: usize = 255;

/// What a container says about itself, beyond the number that identifies it.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Container {
    /// The info hash, lower-case hex.
    pub info_hash: String,
    /// `info.name`, when it is readable text.
    pub name: Option<String>,
    /// `announce` and every entry of `announce-list`, in the order they were written, without
    /// repeats.
    pub trackers: Vec<String>,
}

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

/// The info hash any text carries, wherever in it the `btih` sits.
///
/// Used by `adopt` rather than by intake: what Put.io says about a transfer — its `hash`, its
/// `magneturi`, the `source` it was created from — is three different shapes of one fact, and
/// the question being asked of all three is only ever "is this our twenty bytes".
#[must_use]
pub fn info_hash_within(text: &str) -> Option<String> {
    if let Some(hash) = normalise_info_hash(text) {
        return Some(hash);
    }
    let lower = text.to_ascii_lowercase();
    let start = lower.find("btih:")? + "btih:".len();
    let rest = text.get(start..)?;
    let end = rest
        .find(['&', '/', '?', '#'])
        .unwrap_or_else(|| rest.len().min(40));
    normalise_info_hash(rest.get(..end)?)
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
#[must_use]
pub fn container_info_hash(bytes: &[u8]) -> Option<String> {
    read_container(bytes).map(|container| container.info_hash)
}

/// Everything a container says that the magnet below needs.
///
/// The bytes of `info` are hashed exactly as they lay in the file rather than re-encoded,
/// because a re-encoding that ordered one key differently would produce a different hash for
/// the same torrent — and the whole value of the key is that it is the same number everybody
/// else has.
#[must_use]
pub fn read_container(bytes: &[u8]) -> Option<Container> {
    let root = Dictionary::at(bytes, 0)?;
    let info = root.value(b"info")?;
    let mut hasher = Sha1::new();
    hasher.update(bytes.get(info.clone())?);
    let info_hash = to_hex(&hasher.finalize());
    let name = Dictionary::at(bytes, info.start)
        .and_then(|info| info.value(b"name"))
        .and_then(|range| text_of(bytes, range))
        .map(|name| bounded_name(&name))
        .filter(|name| !name.is_empty());
    let mut trackers = Vec::new();
    if let Some(range) = root.value(b"announce")
        && let Some(tracker) = text_of(bytes, range)
    {
        push_tracker(&mut trackers, tracker);
    }
    // `announce-list` is a list of *tiers*, each a list of addresses. Both levels are walked,
    // because a torrent that puts its only working tracker in the second tier is ordinary.
    if let Some(range) = root.value(b"announce-list") {
        for tier in list_items(bytes, range.start) {
            for entry in list_items(bytes, tier.start) {
                if let Some(tracker) = text_of(bytes, entry) {
                    push_tracker(&mut trackers, tracker);
                }
            }
        }
    }
    Some(Container {
        info_hash,
        name,
        trackers,
    })
}

/// The magnet address a container is equivalent to, or `None` when the bytes are not one.
///
/// This is what `submit` hands to Put.io. It names the same twenty bytes as the file, so the
/// remote job it creates is the same job a magnet for that torrent would have created — which
/// is exactly what the content key promised and what `adopt` relies on after a restart.
#[must_use]
pub fn container_magnet(bytes: &[u8]) -> Option<String> {
    let container = read_container(bytes)?;
    let mut magnet = format!("magnet:?xt=urn:btih:{}", container.info_hash);
    if let Some(name) = &container.name {
        magnet.push_str("&dn=");
        magnet.push_str(&percent_encode(name));
    }
    for tracker in container.trackers.iter().take(MAX_TRACKERS) {
        magnet.push_str("&tr=");
        magnet.push_str(&percent_encode(tracker));
    }
    Some(magnet)
}

/// Percent-encodes everything outside the unreserved set, so a value cannot end the query it
/// sits in — a torrent name containing `&tr=` would otherwise add a tracker of its choosing.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => {
                use std::fmt::Write;
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// `application/x-www-form-urlencoded` body for `POST /v2/transfers/add`.
#[must_use]
pub fn add_body(magnet: &str) -> Vec<u8> {
    format!("url={}", percent_encode(magnet)).into_bytes()
}

/// A tracker address, if it is one, added once.
fn push_tracker(trackers: &mut Vec<String>, candidate: String) {
    let candidate = candidate.trim().to_owned();
    let usable = matches!(
        candidate.split_once("://").map(|(scheme, _)| scheme),
        Some("http" | "https" | "udp" | "ws" | "wss")
    ) && candidate.len() <= 255
        && !candidate.bytes().any(|byte| byte.is_ascii_control());
    if usable && !trackers.contains(&candidate) {
        trackers.push(candidate);
    }
}

/// A torrent name, cut to whole characters at the byte bound and stripped of controls.
fn bounded_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .fold(String::new(), |mut out, character| {
            if out.len() + character.len_utf8() <= MAX_NAME_BYTES {
                out.push(character);
            }
            out
        })
        .trim()
        .to_owned()
}

/// One bencoded dictionary, as a map from key to the byte range of its value.
struct Dictionary {
    entries: Vec<(Vec<u8>, std::ops::Range<usize>)>,
}

impl Dictionary {
    /// Reads the dictionary starting at `at`, or `None` when there is not one there.
    fn at(bytes: &[u8], at: usize) -> Option<Self> {
        if bytes.len() > MAX_CONTAINER_BYTES || bytes.get(at) != Some(&b'd') {
            return None;
        }
        let mut entries = Vec::new();
        let mut cursor = at + 1;
        while *bytes.get(cursor)? != b'e' {
            let (key, after_key) = read_byte_string(bytes, cursor)?;
            let end = skip_value(bytes, after_key, 0)?;
            entries.push((key.to_vec(), after_key..end));
            cursor = end;
        }
        Some(Self { entries })
    }

    fn value(&self, key: &[u8]) -> Option<std::ops::Range<usize>> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, range)| range.clone())
    }
}

/// The byte ranges of the items of the bencoded list at `at`.
fn list_items(bytes: &[u8], at: usize) -> Vec<std::ops::Range<usize>> {
    let mut items = Vec::new();
    if bytes.get(at) != Some(&b'l') {
        return items;
    }
    let mut cursor = at + 1;
    while bytes.get(cursor).is_some_and(|byte| *byte != b'e') {
        let Some(end) = skip_value(bytes, cursor, 0) else {
            break;
        };
        items.push(cursor..end);
        cursor = end;
    }
    items
}

/// The text of the byte string at the start of `range`, when it is one and it is UTF-8.
fn text_of(bytes: &[u8], range: std::ops::Range<usize>) -> Option<String> {
    let (value, _) = read_byte_string(bytes, range.start)?;
    std::str::from_utf8(value).ok().map(str::to_owned)
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

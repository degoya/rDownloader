//! What a torrent source is, read once for every remote-job plugin (RD-191-07, PLUG-07).
//!
//! Six plugins — `putio-transfers`, `seedr-common`, `realdebrid-torrents`, `torbox-jobs`,
//! `offcloud-cloud` and `premiumize-transfers` — each carried a magnet reader, a base32
//! decoder and a bencode walker, and the copies had drifted. The one that mattered:
//! `pair.split_once('=')?` ended the whole magnet at the first pair without a `=`, so
//! `magnet:?dn=x&&xt=urn:btih:…` named no info hash in four plugins while two read on and found
//! it. One magnet was then a remote job at one provider and "not a torrent" at another, and the
//! content key that is meant to stop a provider being asked twice was not derived at all.
//!
//! Everything here is local and target-independent: nothing makes a request, and `cargo test`
//! covers it without a WebAssembly toolchain. The key a provider job is stored under is still
//! each plugin's own — `btih:<hex>`, `torrent:<hex>`, the bare hex — because that is a promise
//! between a plugin and its rows; the number inside it is the same everywhere.
//!
//! For both shapes the number is the BitTorrent info hash, lower-case hex. A magnet carries it
//! (`xt=urn:btih:`), in hex or in base32; a `.torrent` file is the SHA-1 of the bencoded value
//! of its top-level `info` key, which is the same number arrived at the long way.

#![forbid(unsafe_code)]

pub mod bencode;

use std::fmt::Write;

use plugin_common::percent_encode;
use sha1::{Digest, Sha1};

pub use bencode::MAX_CONTAINER_BYTES;

/// Most trackers carried over into a reconstructed magnet. A tracker list is attacker-supplied
/// and an address has to fit in a request; thirty is more than any real torrent lists.
const MAX_TRACKERS: usize = 30;

/// Longest torrent name carried over as `dn`. It is a display name and nothing depends on it,
/// so it is bounded rather than trusted.
const MAX_NAME_BYTES: usize = 255;

/// Longest tracker address carried over.
const MAX_TRACKER_BYTES: usize = 255;

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

/// Whether `address` is a magnet at all, whatever topic it names.
#[must_use]
pub fn is_magnet(address: &str) -> bool {
    magnet_query(address).is_some()
}

/// The info hash a magnet address names, as lower-case hex.
///
/// `None` for anything that is not a magnet, and for a magnet whose `xt` is not a BitTorrent
/// info hash — `urn:sha1:` and `urn:ed2k:` are legal in a magnet and mean something else.
///
/// Tolerant of what people paste: surrounding whitespace, any case of the scheme, empty pairs
/// (`&&`), pairs without a `=` and whitespace around a name or a value are skipped or trimmed
/// rather than ending the read. `xt.1` and friends count too: a magnet may name several topics,
/// and each is a candidate; the first that is a BitTorrent info hash wins.
#[must_use]
pub fn magnet_info_hash(address: &str) -> Option<String> {
    for pair in magnet_query(address)?.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !name.eq_ignore_ascii_case("xt") && !starts_with_ignore_case(name, "xt.") {
            continue;
        }
        let value = value.trim();
        let Some(raw) = strip_prefix_ignore_case(value, "urn:btih:")
            .or_else(|| strip_prefix_ignore_case(value, "urn%3Abtih%3A"))
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

/// The info hash any text carries, wherever in it the `btih` sits.
///
/// For `adopt` rather than intake: what a provider says about a running transfer — a bare
/// hash, a whole magnet, the source it was created from — is several shapes of one fact, and
/// the question asked of each is only ever "is this our twenty bytes".
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

/// The info hash of a bencoded container: SHA-1 over the *encoded* value of its `info` key.
///
/// The bytes are hashed exactly as they lay in the file rather than re-encoded, because a
/// re-encoding that ordered one key differently would produce a different hash for the same
/// torrent — and the whole value of the key is that it is the same number everybody else has.
#[must_use]
pub fn container_info_hash(bytes: &[u8]) -> Option<String> {
    bencode::info_slice(bytes).map(sha1_hex)
}

/// Everything a container says that a magnet for it needs.
#[must_use]
pub fn read_container(bytes: &[u8]) -> Option<Container> {
    let root = bencode::Dictionary::at(bytes, 0)?;
    let info = root.value(b"info")?;
    let info_hash = sha1_hex(bytes.get(info.clone())?);
    let name = bencode::Dictionary::at(bytes, info.start)
        .and_then(|info| info.value(b"name"))
        .and_then(|range| bencode::text_of(bytes, range))
        .map(|name| bounded_name(&name))
        .filter(|name| !name.is_empty());
    let mut trackers = Vec::new();
    if let Some(range) = root.value(b"announce")
        && let Some(tracker) = bencode::text_of(bytes, range)
    {
        push_tracker(&mut trackers, &tracker);
    }
    // `announce-list` is a list of *tiers*, each a list of addresses. Both levels are walked,
    // because a torrent that puts its only working tracker in the second tier is ordinary.
    if let Some(range) = root.value(b"announce-list") {
        for tier in bencode::list_items(bytes, range.start) {
            for entry in bencode::list_items(bytes, tier.start) {
                if let Some(tracker) = bencode::text_of(bytes, entry) {
                    push_tracker(&mut trackers, &tracker);
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

/// The `name` a container's `info` dictionary states, as it stands — lossily decoded, but not
/// yet bounded or cleaned; the caller decides what it is safe for.
#[must_use]
pub fn container_name(bytes: &[u8]) -> Option<String> {
    let info = bencode::info_slice(bytes)?;
    let range = bencode::Dictionary::at(info, 0)?.value(b"name")?;
    let (value, _) = bencode::byte_string(info, range.start)?;
    Some(String::from_utf8_lossy(value).into_owned())
}

/// The magnet address a container is equivalent to, or `None` when the bytes are not one.
///
/// It names the same twenty bytes as the file, so the remote job it creates is the same job a
/// magnet for that torrent would have created — which is exactly what the content key promised
/// and what `adopt` relies on after a restart. The torrent's own name and every tracker it
/// listed travel with it, which is what lets a provider start a torrent whose peers are not
/// reachable over DHT alone.
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

/// Lower-case hex of a SHA-1 over `bytes`.
#[must_use]
pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// Lower-case hex of `bytes`.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing into a String cannot fail; the result is discarded rather than unwrapped.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The query of a magnet address, after `magnet:?` in any case and surrounding whitespace.
fn magnet_query(address: &str) -> Option<&str> {
    strip_prefix_ignore_case(address.trim(), "magnet:?")
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    starts_with_ignore_case(text, prefix).then(|| &text[prefix.len()..])
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.is_char_boundary(prefix.len())
        && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// A tracker address, if it is one, added once.
fn push_tracker(trackers: &mut Vec<String>, candidate: &str) {
    let candidate = candidate.trim();
    let usable = matches!(
        candidate.split_once("://").map(|(scheme, _)| scheme),
        Some("http" | "https" | "udp" | "ws" | "wss")
    ) && candidate.len() <= MAX_TRACKER_BYTES
        && !candidate.bytes().any(|byte| byte.is_ascii_control());
    if usable && !trackers.iter().any(|known| known == candidate) {
        trackers.push(candidate.to_owned());
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
mod tests;

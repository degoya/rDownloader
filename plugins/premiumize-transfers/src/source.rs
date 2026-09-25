//! What a person handed over, and the key that stands for it.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point --
//! the content key is what keeps a provider whose `transfer/create` is not idempotent from
//! being asked twice, and a key that needed a request could not be written down before the
//! first one. See `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! Premiumize takes all three shapes `job-source` knows, so all three need a key, and the key
//! says which shape it came from:
//!
//! | Shape | Key | Why |
//! | --- | --- | --- |
//! | a magnet naming a BitTorrent info hash | `btih:<40 hex>` | the number everybody else has, so one torrent pasted from two sites is one transfer |
//! | any other magnet | `magnet:<sha1 of the address>` | `urn:sha1:` and `urn:ed2k:` are legal in a magnet and mean something else |
//! | a plain address | `url:<sha1 of the trimmed address>` | the address is what was handed over |
//! | a container | `file:<sha1 of the bytes>` | see below |
//!
//! A container is keyed by its **bytes** and not by an info hash. Reading a `.torrent`'s
//! `info` dictionary would give the number a magnet for the same torrent produces, which is
//! what `plugins/realdebrid-torrents/` does -- but Premiumize also takes `.nzb`, `.dlc` and
//! `.rsdf`, which have no such number at all, and a rule that held for one of four shapes
//! would be a rule nobody could state. Two byte-identical uploads are one key, which is the
//! case the guard exists for; the same torrent as a file and as a magnet are two, and that is
//! recorded as the cost rather than hidden.

use sha1::{Digest, Sha1};

pub use premiumize_common::container::MAX_CONTAINER_BYTES;

/// Longest address this plugin will look at. The host bounds a submitted source already; this
/// bounds what `claims` and `identify` scan, which run before anything is handed over.
pub const MAX_ADDRESS_BYTES: usize = 8 * 1024;

/// The key of a magnet address, or `None` when it is not a magnet at all.
#[must_use]
pub fn magnet_key(address: &str) -> Option<String> {
    let address = address.trim();
    if address.len() > MAX_ADDRESS_BYTES {
        return None;
    }
    let rest = address
        .strip_prefix("magnet:?")
        .or_else(|| address.strip_prefix("MAGNET:?"))?;
    if let Some(hash) = info_hash(rest) {
        return Some(format!("btih:{hash}"));
    }
    Some(format!("magnet:{}", digest(address.as_bytes())))
}

/// The key of a plain address Premiumize would fetch for itself.
///
/// Only `http` and `https`: the host checks the scheme before a source ever reaches a plugin,
/// and this is the second half of the same rule, so a `file:` address is not this plugin's
/// whatever else happens.
#[must_use]
pub fn address_key(address: &str) -> Option<String> {
    let address = address.trim();
    if address.len() > MAX_ADDRESS_BYTES {
        return None;
    }
    let lowered = address.to_ascii_lowercase();
    if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
        return None;
    }
    // A bare scheme is not an address. Anything beyond that is the provider's to judge: it
    // fetches links this installation has never heard of, which is the point of handing it one.
    let rest = &lowered[lowered.find("//")? + 2..];
    (!rest.is_empty()).then(|| format!("url:{}", digest(address.as_bytes())))
}

/// The key of a container, or `None` for one this plugin cannot name an upload for.
#[must_use]
pub fn container_key(bytes: &[u8]) -> Option<String> {
    premiumize_common::container::extension(bytes)?;
    Some(format!("file:{}", digest(bytes)))
}

/// The BitTorrent info hash a magnet's query names, as lower-case hex.
fn info_hash(query: &str) -> Option<String> {
    for pair in query.split('&') {
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
/// means a magnet copied from two different sites produces one transfer and not two.
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

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
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
            // The consumed bits are dropped rather than left in the accumulator. Without this
            // it grows by five bits per character and overflows on the thirteenth, which in a
            // debug build is a panic and in a release build a wrong answer.
            accumulator &= (1_u64 << bits) - 1;
        }
    }
    (bytes.len() == 20).then_some(bytes)
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

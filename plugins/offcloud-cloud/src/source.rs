//! What a person handed over, and the key that stands for it.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point —
//! the content key is what keeps a provider whose submit is not idempotent from being asked
//! twice, and a key that needed a request could not be written down before the first one. See
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! Offcloud's cloud takes one field, `url`, and it takes a magnet there as readily as an
//! ordinary address. That is why this file derives two kinds of key and keeps them apart:
//!
//! - a **magnet** is keyed by its BitTorrent info hash, so the same content copied from two
//!   sites in two spellings is one job and not two;
//! - an **address** has no such number, so it is keyed by the SHA-1 of the address itself
//!   after a light normalisation.
//!
//! The two carry different prefixes — `btih:` and `url:` — for a reason the unique index makes
//! unforgiving: `remote_jobs(account_id, content_key)` is a single space, and a bare hex digest
//! from either derivation would be forty characters in it. Without the prefix an address whose
//! SHA-1 happened to equal a magnet's info hash would be refused as a duplicate of a job it has
//! nothing to do with.

use sha1::{Digest, Sha1};

/// How a magnet's key is spelled.
pub const MAGNET_PREFIX: &str = "btih:";
/// How an address's key is spelled.
pub const ADDRESS_PREFIX: &str = "url:";

/// Longest address this plugin will key. Far past any real one; a value beyond it is not an
/// address and hashing it would spend the invocation's budget finding that out.
pub const MAX_ADDRESS_BYTES: usize = 8 * 1024;

/// The content key of a magnet, or `None` when it names no BitTorrent info hash.
///
/// `urn:sha1:` and `urn:ed2k:` are legal in a magnet and mean something else, so a magnet
/// carrying one of those is not this plugin's.
#[must_use]
pub fn magnet_key(address: &str) -> Option<String> {
    magnet_info_hash(address).map(|hash| format!("{MAGNET_PREFIX}{hash}"))
}

/// The content key of an ordinary address, or `None` when it is not one this plugin takes.
///
/// Only `http` and `https`: the host checks the scheme before a source ever reaches a plugin,
/// and this is the second half of the same rule rather than a repetition of it — what is
/// refused here is an address that *is* http-shaped and still cannot be a job, such as one
/// with no host at all.
#[must_use]
pub fn address_key(address: &str) -> Option<String> {
    let normalised = normalise_address(address)?;
    let mut hasher = Sha1::new();
    hasher.update(normalised.as_bytes());
    Some(format!("{ADDRESS_PREFIX}{}", to_hex(&hasher.finalize())))
}

/// The address as it is keyed: trimmed, with the scheme and host lower-cased and a trailing
/// fragment dropped.
///
/// Deliberately conservative. A fragment never reaches the server, so two addresses that differ
/// only in one are the same download and must be the same job. Everything else — the query, the
/// path's case, a trailing slash — is left exactly as it arrived, because at some hosters those
/// are different files and a normalisation that merged them would refuse the second one as a
/// duplicate of the first.
#[must_use]
pub fn normalise_address(address: &str) -> Option<String> {
    let address = address.trim();
    if address.is_empty() || address.len() > MAX_ADDRESS_BYTES {
        return None;
    }
    let (scheme, rest) = address.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or(rest);
    // Everything up to the first `/`, `?` is the authority, and it is case-insensitive.
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}{tail}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

/// The info hash a magnet address names, as lower-case hex.
#[must_use]
pub fn magnet_info_hash(address: &str) -> Option<String> {
    let rest = address
        .trim()
        .strip_prefix("magnet:?")
        .or_else(|| address.trim().strip_prefix("MAGNET:?"))?;
    for pair in rest.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
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

/// The content key of whatever Offcloud recorded as the original link of a job it already
/// holds.
///
/// The other half of the adoption check: the account's own history says what each job was
/// started from, and that string is put through exactly the derivation a fresh source would
/// have been put through. A magnet in the history is keyed as a magnet; anything else that is
/// an address is keyed as an address.
#[must_use]
pub fn key_of_original_link(link: &str) -> Option<String> {
    magnet_key(link).or_else(|| address_key(link))
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

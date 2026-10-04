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

// The magnet reader is `torrent-common`'s, the same code every remote-job plugin runs
// (RD-191-07).
use torrent_common::sha1_hex;
pub use torrent_common::{magnet_info_hash, normalise_info_hash};

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
    Some(format!(
        "{ADDRESS_PREFIX}{}",
        sha1_hex(normalised.as_bytes())
    ))
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

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

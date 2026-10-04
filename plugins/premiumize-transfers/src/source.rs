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

// The magnet reader is `torrent-common`'s, the same code every remote-job plugin runs
// (RD-191-07).
pub use torrent_common::normalise_info_hash;
use torrent_common::{is_magnet, magnet_info_hash, sha1_hex as digest};

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
    if !is_magnet(address) {
        return None;
    }
    if let Some(hash) = magnet_info_hash(address) {
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

/// The token a container's upload name carries: the first twelve hex digits of the digest its
/// key is made of.
///
/// Premiumize names a transfer, and the folder it finishes into, after the uploaded file's
/// name, so two uploads under one name share a folder and each job reads the other's files
/// back (owner report, 2026-09-27). Derived from the bytes, so two containers never share a
/// name and a container sent again is named as it was the first time.
#[must_use]
pub fn upload_tag(bytes: &[u8]) -> String {
    digest(bytes).chars().take(12).collect()
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

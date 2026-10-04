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

use plugin_common::percent_encode;

// The magnet reader, the bencode walker and the container magnet are `torrent-common`'s, the
// same code every remote-job plugin runs (RD-191-07); they are re-exported here because this
// module is where the rest of the plugin has always found them.
pub use torrent_common::{
    Container, MAX_CONTAINER_BYTES, container_info_hash, container_magnet, info_hash_within,
    magnet_info_hash, normalise_info_hash, read_container,
};

/// `application/x-www-form-urlencoded` body for `POST /v2/transfers/add`.
#[must_use]
pub fn add_body(magnet: &str) -> Vec<u8> {
    format!("url={}", percent_encode(magnet)).into_bytes()
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

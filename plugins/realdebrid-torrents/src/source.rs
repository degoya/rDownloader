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

// The magnet reader and the bencode walker are `torrent-common`'s, the same code every
// remote-job plugin runs (RD-191-07).
pub use torrent_common::{
    MAX_CONTAINER_BYTES, container_info_hash, magnet_info_hash, normalise_info_hash,
};

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

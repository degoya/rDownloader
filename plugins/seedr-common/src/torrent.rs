//! What a person handed over, and the number that identifies it.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point —
//! the content key is what keeps a provider whose submit is not idempotent from being asked
//! twice, and a key that needed a request could not be written down before the first one. See
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! For both shapes the number is the BitTorrent info hash, lower-case hex. A magnet carries it
//! (`xt=urn:btih:`), in hex or in base32; a `.torrent` file is the SHA-1 of the bencoded value
//! of its top-level `info` key, which is the same number arrived at the long way. Being the
//! same number for both shapes is what lets a magnet and the matching file find the same
//! remote job — and what lets `adopt` recognise it in what Seedr says about a transfer.
//!
//! **A container is submitted as a magnet.** Seedr documents `POST /rest/transfer/file`, a
//! multipart upload, beside `POST /rest/transfer/magnet`. This plugin takes the second for
//! both: [`container_magnet`] reads the container once, locally, and writes the magnet it is
//! equivalent to — the same info hash, the torrent's own name, and every tracker the file
//! listed. What that costs is a torrent carrying neither trackers nor peers reachable over
//! DHT, which Seedr then cannot start; what it buys is one request shape instead of two, one
//! body builder instead of a multipart writer, and a content key that is unchanged by which of
//! the two shapes somebody pasted. `plugins/putio-transfers/` reached the same answer for the
//! same reason.

use plugin_common::percent_encode;

// The magnet reader, the bencode walker and the container magnet are `torrent-common`'s, the
// same code every remote-job plugin runs (RD-191-07); re-exported here, where both Seedr
// plugins have always found them.
pub use torrent_common::{
    Container, MAX_CONTAINER_BYTES, container_info_hash, container_magnet, info_hash_within,
    magnet_info_hash, normalise_info_hash, read_container,
};

/// `application/x-www-form-urlencoded` body for `POST /rest/transfer/magnet`.
///
/// Encoded by hand rather than with a URL crate for one field, because that field is a magnet
/// — full of `&`, `=` and `:` — and a body that did not encode them would submit a truncated
/// address and create a transfer for something nobody asked for.
#[must_use]
pub fn add_magnet_body(magnet: &str) -> Vec<u8> {
    format!("magnet={}", percent_encode(magnet)).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::{add_magnet_body, magnet_info_hash};

    /// PLUG-07: an empty pair (`&&`) used to end the whole read, so a good magnet had no key
    /// and Seedr was never asked.
    #[test]
    fn an_empty_pair_does_not_hide_the_info_hash() {
        assert_eq!(
            magnet_info_hash("magnet:?dn=Example&&xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ")
                .as_deref(),
            Some("da39a3ee5e6b4b0d3255bfef95601890afd80709")
        );
    }

    /// A magnet is full of the separators of the document it lands in, and a body that did not
    /// encode them would submit a truncated address.
    #[test]
    fn the_form_body_encodes_the_magnet_whole() {
        let body = String::from_utf8(add_magnet_body(
            "magnet:?xt=urn:btih:da39&dn=A B&tr=udp://t.invalid",
        ))
        .expect("utf-8");
        assert!(
            body.starts_with("magnet=magnet%3A%3Fxt%3Durn%3Abtih%3Ada39"),
            "{body}"
        );
        assert_eq!(body.matches('&').count(), 0, "{body}");
    }
}

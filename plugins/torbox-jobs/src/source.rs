//! What a person handed over, which of TorBox's three job kinds it is, and the key that
//! stands for it.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point --
//! the content key is what keeps a provider whose create calls are not idempotent from being
//! asked twice, and a key that needed a request could not be written down before the first
//! one. See `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! TorBox runs three kinds of job behind three sets of endpoints, and the contract hands this
//! plugin three shapes of source. They line up, but not one to one: a container is a
//! `.torrent` *or* an `.nzb`, and which one it is has to be read out of the bytes.
//!
//! | Source | Kind | Key |
//! | --- | --- | --- |
//! | `magnet:?xt=urn:btih:` | [`Kind::Torrent`] | the BitTorrent info hash, lower-case hex |
//! | bencoded container | [`Kind::Torrent`] | SHA-1 of the encoded `info` value |
//! | an `.nzb` document | [`Kind::Usenet`] | MD5 of the container's bytes |
//! | an `http(s)` address | [`Kind::Web`] | MD5 of the address |
//!
//! **The three digests are TorBox's own.** They are what its `checkcached` endpoints take and
//! what its `mylist` entries carry in `hash`, which is the only reason this plugin can
//! recognise a job it created seconds before a crash. They are derived here rather than asked
//! for, because a key that needed a request could not exist before the request it protects.
//!
//! The key carries the kind in front of the digest -- `torrent:<hex>` -- for two reasons. A
//! `poll` knows which endpoints to use without a lookup, an `adopt` knows which list to read,
//! and the account's unique index cannot collide an NZB's digest with a link's.

use md5::{Digest, Md5};
// The magnet reader and the bencode walker are `torrent-common`'s, the same code every
// remote-job plugin runs (RD-191-07).
use torrent_common::to_hex;
pub use torrent_common::{container_info_hash, magnet_info_hash, normalise_info_hash};

/// Longest container this plugin will read. A `.torrent` is kilobytes and an `.nzb` for a
/// large release is megabytes; anything far past that is neither, and scanning it would spend
/// the invocation's whole budget finding that out.
pub const MAX_CONTAINER_BYTES: usize = 8 * 1024 * 1024;

/// Longest address this plugin will take. Far above any real download link, and low enough
/// that a megabyte of `data:` pasted into the box is refused rather than submitted.
pub const MAX_ADDRESS_BYTES: usize = 2048;

/// How far into a container the `<nzb` element is looked for.
const NZB_SNIFF_BYTES: usize = 4096;

/// Which of TorBox's three job kinds a source belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// `/torrents/*`: a magnet or a `.torrent` file.
    Torrent,
    /// `/usenet/*`: an `.nzb` document.
    Usenet,
    /// `/webdl/*`: an ordinary address TorBox fetches for itself.
    Web,
}

impl Kind {
    /// The word that travels in the content key and in the handle's `job-state`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Torrent => "torrent",
            Self::Usenet => "usenet",
            Self::Web => "web",
        }
    }

    /// The kind that word names, or `None` for anything else.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "torrent" => Some(Self::Torrent),
            "usenet" => Some(Self::Usenet),
            "web" => Some(Self::Web),
            _ => None,
        }
    }
}

/// A source as the contract hands it over, without the WIT types.
///
/// Mirrored rather than used directly so that everything in this module compiles and is tested
/// on an ordinary host, where there is no WebAssembly target and no generated bindings.
#[derive(Clone, Copy, Debug)]
pub enum Handed<'a> {
    Magnet(&'a str),
    Container(&'a [u8]),
    Address(&'a str),
}

/// The kind and the content key of a source, or `None` when it is not one of TorBox's.
#[must_use]
pub fn identify(source: Handed<'_>) -> Option<(Kind, String)> {
    let (kind, digest) = match source {
        Handed::Magnet(address) => (Kind::Torrent, magnet_info_hash(address)?),
        Handed::Container(bytes) => container_kind_and_digest(bytes)?,
        Handed::Address(address) => (Kind::Web, address_digest(address)?),
    };
    Some((kind, content_key(kind, &digest)))
}

/// The digest TorBox's `checkcached` for `kind` knows a source by, or `None` when that cache
/// cannot be asked about it (RD-130-11).
///
/// Chosen by the kind the host decided, not by the source's shape: an NZB link and a hoster
/// link are both addresses, and only the kind says which of the three caches to ask. The
/// digests are the same ones [`identify`] derives -- TorBox's own -- so a cache answer and a
/// job created afterwards name the content alike.
///
/// | Kind | Source | Digest |
/// | --- | --- | --- |
/// | [`Kind::Torrent`] | magnet | the info hash, lower-case hex |
/// | [`Kind::Torrent`] | bencoded container | SHA-1 of the encoded `info` value |
/// | [`Kind::Usenet`] | address | MD5 of the address exactly as handed over |
/// | [`Kind::Usenet`] | `.nzb` container | MD5 of the bytes |
/// | [`Kind::Web`] | address | MD5 of the address exactly as handed over |
///
/// Anything else -- a magnet asked about as a hoster link, a container as a web download --
/// has no digest here and is answered `unknown` without a request.
#[must_use]
pub fn cache_digest(kind: Kind, source: Handed<'_>) -> Option<String> {
    match (kind, source) {
        (Kind::Torrent, Handed::Magnet(address)) => magnet_info_hash(address),
        (Kind::Torrent, Handed::Container(bytes)) => container_info_hash(bytes),
        (Kind::Usenet, Handed::Container(bytes))
            if !bytes.is_empty() && bytes.len() <= MAX_CONTAINER_BYTES && is_nzb(bytes) =>
        {
            Some(md5_hex(bytes))
        }
        (Kind::Usenet | Kind::Web, Handed::Address(address)) => address_digest(address),
        _ => None,
    }
}

/// `<kind>:<digest>`.
#[must_use]
pub fn content_key(kind: Kind, digest: &str) -> String {
    format!("{}:{digest}", kind.as_str())
}

/// The kind and the digest a content key carries, or `None` when it is not one of ours.
///
/// Checked rather than split: the key comes back out of the host's own row, and a digest with
/// a slash in it would be a request to somewhere else on the one host this plugin may reach.
#[must_use]
pub fn split_key(key: &str) -> Option<(Kind, &str)> {
    let (word, digest) = key.split_once(':')?;
    let kind = Kind::parse(word)?;
    let plausible =
        matches!(digest.len(), 32 | 40) && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
    plausible.then_some((kind, digest))
}

/// Which kind a container is, and its digest.
pub(crate) fn container_kind_and_digest(bytes: &[u8]) -> Option<(Kind, String)> {
    if bytes.len() > MAX_CONTAINER_BYTES || bytes.is_empty() {
        return None;
    }
    if let Some(hash) = container_info_hash(bytes) {
        return Some((Kind::Torrent, hash));
    }
    is_nzb(bytes).then(|| (Kind::Usenet, md5_hex(bytes)))
}

/// Whether a container is an NZB document.
///
/// Sniffed rather than parsed: an NZB is XML whose root element is `<nzb`, and finding that
/// element is the whole question. Parsing the document to answer it would mean carrying an XML
/// reader into a sandbox for a decision a substring settles -- and TorBox, not this plugin, is
/// what has to be able to read it.
#[must_use]
pub fn is_nzb(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(NZB_SNIFF_BYTES)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    text.contains("<nzb")
}

/// The digest of a web address: MD5 over the address exactly as it will be submitted.
///
/// "Exactly as submitted" is the contract with itself that makes the key useful. TorBox hashes
/// the link it was given; normalising here -- lower-casing a path, dropping a trailing slash --
/// would produce a key for an address nobody sent, and the adoption that key exists for would
/// never match.
#[must_use]
pub fn address_digest(address: &str) -> Option<String> {
    let address = address.trim();
    if address.len() > MAX_ADDRESS_BYTES || address.is_empty() {
        return None;
    }
    if address.bytes().any(|byte| byte <= b' ' || byte == 0x7f) {
        return None;
    }
    let lowered = address.to_ascii_lowercase();
    // `http` and `https` and nothing else. The host refuses a `file:` or a `data:` address
    // before it ever reaches a plugin; answering the same way here means the two cannot drift.
    if !lowered.starts_with("http://") && !lowered.starts_with("https://") {
        return None;
    }
    Some(md5_hex(address.as_bytes()))
}

fn md5_hex(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;

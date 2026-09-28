//! Which source a link stands for, spelled so two spellings of the same source compare equal
//! (RD-150-01).
//!
//! A **source duplicate** is the same thing asked for twice — the same address, the same
//! torrent, the same NZB, the same file at a hoster — and is known before a byte is fetched. A
//! **content duplicate** is the same bytes arriving from wherever, and is known only once a
//! file is hashed. The two are explained separately on purpose: a source duplicate says "this
//! is already queued", a content duplicate says "these bytes are already on disk".
//!
//! The normalisation is deliberately conservative. Only differences that provably do not change
//! *which* source is meant are removed; a query string stays, because on plenty of hosts it is
//! the whole address of the file.

use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

/// Which rule produced the identity.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceIdentityKind {
    /// A plain address, normalised.
    Url,
    /// A BitTorrent info-hash, whatever trackers and names the magnet carried.
    Magnet,
    /// An NZB by the SHA-256 of its document, plus the file inside it.
    Nzb,
    /// A file at a hoster the provider registry knows, by provider and address.
    Provider,
}

/// A source, normalised. Two links with equal identities ask for the same thing.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, ToSchema)]
pub struct SourceIdentity {
    pub kind: SourceIdentityKind,
    pub key: String,
}

impl SourceIdentity {
    /// The identity of an address: a magnet by its info-hash, anything else normalised.
    #[must_use]
    pub fn of_url(url: &Url) -> Self {
        if url.scheme() == "magnet"
            && let Some(hash) = magnet_info_hash(url)
        {
            return Self {
                kind: SourceIdentityKind::Magnet,
                key: hash,
            };
        }
        Self {
            kind: SourceIdentityKind::Url,
            key: normalized_url(url),
        }
    }

    /// A file inside an NZB. The document hash is what intake already keys imports on; the
    /// name tells two files of the same NZB apart.
    #[must_use]
    pub fn nzb(document_sha256: &str, file_name: &str) -> Self {
        Self {
            kind: SourceIdentityKind::Nzb,
            key: format!(
                "{}/{}",
                document_sha256.trim().to_ascii_lowercase(),
                file_name.trim()
            ),
        }
    }

    /// A file at a known hoster. `url` should already be the canonical address the collector
    /// rewrites aliases to, so `ddl.to` and `ddownload.com` meet here.
    #[must_use]
    pub fn provider(slug: &str, url: &Url) -> Self {
        Self {
            kind: SourceIdentityKind::Provider,
            key: format!("{}:{}", slug.trim(), normalized_url(url)),
        }
    }
}

/// An address without the parts that do not change what it points at: the fragment, the case
/// of the host, a leading `www.`, the default port, `http` against `https`, and a trailing
/// slash on the path.
#[must_use]
pub fn normalized_url(url: &Url) -> String {
    let scheme = match url.scheme() {
        "http" => "https",
        other => other,
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    // `port()` is already `None` for the scheme's default port.
    let port = url
        .port()
        .filter(|port| !(scheme == "https" && *port == 443))
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = url.path();
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        ""
    };
    let query = url
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    if url.cannot_be_a_base() {
        return format!("{scheme}:{}{query}", url.path());
    }
    format!("{scheme}://{host}{port}{path}{query}")
}

/// The info-hash of a magnet link as lowercase hex, `btih:` or `btmh:` prefixed.
///
/// A v1 hash comes as 40 hex digits or 32 base32 characters; both spellings of the same
/// torrent must meet, so the base32 form is decoded. The first `xt` that parses wins.
#[must_use]
pub fn magnet_info_hash(url: &Url) -> Option<String> {
    url.query_pairs()
        .filter(|(key, _)| key == "xt")
        .find_map(|(_, value)| {
            let value = value.to_ascii_lowercase();
            if let Some(hash) = value.strip_prefix("urn:btih:") {
                return v1_hash(hash).map(|hex| format!("btih:{hex}"));
            }
            let hash = value.strip_prefix("urn:btmh:")?;
            (!hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| format!("btmh:{hash}"))
        })
}

fn v1_hash(hash: &str) -> Option<String> {
    if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(hash.to_owned());
    }
    if hash.len() == 32 {
        return decode_base32(hash).map(hex::encode);
    }
    None
}

/// RFC 4648 base32 without padding, the alphabet magnet links use. Case was folded by the
/// caller.
fn decode_base32(value: &str) -> Option<Vec<u8>> {
    let mut bits: u64 = 0;
    let mut count = 0_u32;
    let mut out = Vec::with_capacity(value.len() * 5 / 8);
    for byte in value.bytes() {
        let digit = match byte {
            b'a'..=b'z' => byte - b'a',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => return None,
        };
        bits = (bits << 5) | u64::from(digit);
        count += 5;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from((bits >> count) & 0xff).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{SourceIdentity, SourceIdentityKind, magnet_info_hash, normalized_url};

    fn url(value: &str) -> Url {
        Url::parse(value).expect("url")
    }

    #[test]
    fn cosmetic_differences_do_not_make_a_second_source() {
        let plain = normalized_url(&url("https://example.com/file.bin?id=7"));
        for spelling in [
            "http://example.com/file.bin?id=7",
            "https://WWW.Example.com/file.bin?id=7",
            "https://example.com:443/file.bin?id=7",
            "https://example.com/file.bin/?id=7",
            "https://example.com/file.bin?id=7#part",
        ] {
            assert_eq!(normalized_url(&url(spelling)), plain, "{spelling}");
        }
    }

    #[test]
    fn a_query_is_part_of_the_address() {
        assert_ne!(
            normalized_url(&url("https://example.com/get?id=1")),
            normalized_url(&url("https://example.com/get?id=2"))
        );
    }

    #[test]
    fn a_magnet_is_its_info_hash_whatever_else_it_carries() {
        let hex = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
        let first = url(&format!(
            "magnet:?xt=urn:btih:{hex}&dn=Name&tr=udp://one.example:80"
        ));
        let second = url(&format!(
            "magnet:?dn=Other&xt=urn:btih:{}&tr=udp://two.example:80",
            hex.to_ascii_uppercase()
        ));
        assert_eq!(
            SourceIdentity::of_url(&first),
            SourceIdentity::of_url(&second)
        );
        assert_eq!(
            SourceIdentity::of_url(&first).kind,
            SourceIdentityKind::Magnet
        );
    }

    #[test]
    fn the_base32_spelling_of_a_v1_hash_meets_the_hex_one() {
        // 20 bytes of 0x00..0x13 in both spellings.
        let bytes: Vec<u8> = (0_u8..20).collect();
        let hex = hex::encode(&bytes);
        let base32 = "AAAQEAYEAUDAOCAJBIFQYDIOB4IBCEQT";
        let from_hex = magnet_info_hash(&url(&format!("magnet:?xt=urn:btih:{hex}")));
        let from_base32 = magnet_info_hash(&url(&format!("magnet:?xt=urn:btih:{base32}")));
        assert_eq!(from_hex, Some(format!("btih:{hex}")));
        assert_eq!(from_base32, from_hex);
    }

    #[test]
    fn a_magnet_without_a_usable_hash_falls_back_to_its_address() {
        let broken = url("magnet:?xt=urn:btih:nothex&dn=x");
        assert_eq!(magnet_info_hash(&broken), None);
        assert_eq!(
            SourceIdentity::of_url(&broken).kind,
            SourceIdentityKind::Url
        );
    }

    #[test]
    fn nzb_and_provider_identities_name_their_parts() {
        let nzb = SourceIdentity::nzb("ABCDEF", "part01.rar");
        assert_eq!(nzb.key, "abcdef/part01.rar");
        let provider = SourceIdentity::provider("ddownload", &url("http://ddownload.com/abc"));
        assert_eq!(provider.key, "ddownload:https://ddownload.com/abc");
    }
}

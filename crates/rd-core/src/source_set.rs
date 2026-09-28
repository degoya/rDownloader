//! The sources of one file, and how each of them is doing (RD-150-03).
//!
//! A Metalink document names one file and every place it can be fetched from, with a
//! priority, a location and the hashes the bytes must match. [`SourceSet`] is that statement
//! after the host has checked it: the addresses parse, carry a scheme a runner exists for and
//! no password, the hashes are hex of the right length, and the piece list covers exactly the
//! stated size. What a plugin proposed and this module refused is dropped, never repaired.
//!
//! [`DownloadSource`] is the persisted half: one source of one queued download, its place in
//! the order and its health. The order is fixed when the row is written and never recomputed,
//! so a restart walks the sources exactly as the attempt before it did.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{ChecksumAlgorithm, ExpectedChecksum};

/// Most sources kept for one file. A mirror list longer than this is a crawler's output, not
/// an author's, and every source costs a probe per attempt.
pub const MAX_SOURCES: usize = 32;
/// Most piece hashes kept for one file. 65 536 SHA-256 pieces of 16 MiB cover a terabyte.
pub const MAX_PIECES: usize = 65_536;
/// Smallest piece length accepted. Anything smaller turns verification into a read per page.
pub const MIN_PIECE_LENGTH: u64 = 16 * 1024;
/// Largest piece length accepted; a chunk is always at least one piece long.
pub const MAX_PIECE_LENGTH: u64 = 1024 * 1024 * 1024;
/// Longest address kept for a source.
pub const MAX_SOURCE_URL: usize = 4096;

/// Stable code: a piece a source delivered did not match its stated hash.
pub const CODE_PIECE_MISMATCH: &str = "mirror.piece_hash_mismatch";
/// Stable code: a source answers with a different size than the set states.
pub const CODE_SOURCE_SIZE_MISMATCH: &str = "mirror.size_mismatch";
/// Stable code: every source of the set is isolated or waiting out a backoff.
pub const CODE_NO_USABLE_SOURCE: &str = "mirror.no_usable_source";
/// Stable code: a source points at this machine or, for a set that came from elsewhere, at the
/// person's own network; it is not requested (RD-150-03).
pub const CODE_INTERNAL_ADDRESS: &str = "mirror.internal_address";

/// Shortest wait after a source failed once.
const BACKOFF_BASE_SECONDS: i64 = 30;
/// Longest wait a source is ever put on, however often it failed.
const BACKOFF_MAX_SECONDS: i64 = 30 * 60;

/// How the host reaches a source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceProtocol {
    Http,
    Https,
    Ftp,
    Ftps,
    Sftp,
}

impl SourceProtocol {
    /// The protocol a scheme stands for, or `None` for one no runner speaks.
    #[must_use]
    pub fn from_scheme(scheme: &str) -> Option<Self> {
        match scheme.to_ascii_lowercase().as_str() {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "ftp" => Some(Self::Ftp),
            "ftps" => Some(Self::Ftps),
            "sftp" => Some(Self::Sftp),
            _ => None,
        }
    }

    /// The persisted spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Ftp => "ftp",
            Self::Ftps => "ftps",
            Self::Sftp => "sftp",
        }
    }

    /// Whether chunks of one file may be fetched from this source in parallel with others.
    ///
    /// Only HTTP today: its range requests are what the chunk engine is built on. An FTP or
    /// SFTP source is kept, ordered and shown, and waits for its runner to learn ranges.
    #[must_use]
    pub const fn serves_chunks(self) -> bool {
        matches!(self, Self::Http | Self::Https)
    }
}

/// One address a set names for its file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SetSource {
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    /// Lower is preferred, as Metalink 4 counts it; `None` sorts after every ranked source.
    pub priority: Option<u32>,
    /// ISO 3166-1 alpha-2 code the document gave, lowercase.
    pub location: Option<String>,
}

/// Hashes of consecutive, equally long pieces of the file; the last piece may be shorter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PieceHashes {
    pub algorithm: ChecksumAlgorithm,
    pub length: u64,
    /// Lowercase hex, one per piece, in file order.
    pub hashes: Vec<String>,
}

impl PieceHashes {
    /// The byte range piece `index` covers in a file of `total` bytes.
    #[must_use]
    pub fn range(&self, index: usize, total: u64) -> (u64, u64) {
        let start = (index as u64).saturating_mul(self.length);
        (
            start.min(total),
            start.saturating_add(self.length).min(total),
        )
    }
}

/// Every source of one file, with what its bytes must hash to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SourceSet {
    /// In the order they are tried: priority first, then the order the document gave.
    pub sources: Vec<SetSource>,
    /// Size the document states, when it does.
    pub size: Option<u64>,
    /// The strongest whole-file hash the document states.
    pub checksum: Option<ExpectedChecksum>,
    pub pieces: Option<PieceHashes>,
    /// Whether the sources may point into the person's own network (RFC 1918, IPv6
    /// unique-local). Only a set the person handed over themselves — pasted, not relayed from
    /// a web page or another program — may; a set from anywhere else is held to public
    /// addresses. This machine is out of reach either way.
    #[serde(default)]
    pub local_network: bool,
}

/// A hash as a plugin handed it over, before it is checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatedHash {
    pub algorithm: String,
    pub value: String,
}

impl SourceSet {
    /// Builds a checked set, or `None` when no source survives the checks.
    ///
    /// `sources` is `(url, priority, location)` in document order.
    #[must_use]
    pub fn checked(
        sources: impl IntoIterator<Item = (String, Option<u32>, Option<String>)>,
        size: Option<u64>,
        hashes: &[StatedHash],
        pieces: Option<(String, u64, Vec<String>)>,
    ) -> Option<Self> {
        let mut kept: Vec<(usize, SetSource)> = Vec::new();
        for (index, (address, priority, location)) in sources.into_iter().enumerate() {
            if kept.len() >= MAX_SOURCES {
                break;
            }
            let Some(url) = checked_url(&address) else {
                continue;
            };
            // The same address twice is one source; the first mention keeps its rank.
            if kept.iter().any(|(_, source)| source.url == url) {
                continue;
            }
            kept.push((
                index,
                SetSource {
                    url,
                    priority: priority.filter(|value| (1..=999_999).contains(value)),
                    location: location.and_then(|value| checked_location(&value)),
                },
            ));
        }
        if kept.is_empty() {
            return None;
        }
        // Stable: equal priorities keep the order the author wrote them in.
        kept.sort_by_key(|(index, source)| (source.priority.unwrap_or(u32::MAX), *index));
        let checksum = strongest(hashes);
        let pieces = pieces.and_then(|(algorithm, length, hashes)| {
            checked_pieces(&algorithm, length, hashes, size)
        });
        Some(Self {
            sources: kept.into_iter().map(|(_, source)| source).collect(),
            size,
            checksum,
            pieces,
            // Decided by where the document came from, which the intake knows and this does
            // not; the strict answer until it says otherwise.
            local_network: false,
        })
    }

    /// Whether the set carries something that proves the bytes, which is the condition for
    /// mixing chunks from several sources into one file.
    #[must_use]
    pub fn has_hash_basis(&self) -> bool {
        self.checksum.is_some() || self.pieces.is_some()
    }
}

/// An address a source may carry: a known scheme, a host, no password, bounded length.
///
/// A password in a mirror address would be written to the database and shown in the queue;
/// a source that needs a login uses the stored remote credentials instead.
fn checked_url(address: &str) -> Option<Url> {
    let address = address.trim();
    if address.is_empty() || address.len() > MAX_SOURCE_URL {
        return None;
    }
    let url = Url::parse(address).ok()?;
    SourceProtocol::from_scheme(url.scheme())?;
    if url.host_str().is_none_or(str::is_empty) || url.password().is_some() {
        return None;
    }
    Some(url)
}

fn checked_location(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic()))
        .then(|| value.to_ascii_lowercase())
}

/// The algorithm a Metalink hash type names, when it is one the host computes.
#[must_use]
pub fn metalink_algorithm(name: &str) -> Option<ChecksumAlgorithm> {
    match name.trim().to_ascii_lowercase().replace('-', "").as_str() {
        "sha256" => Some(ChecksumAlgorithm::Sha256),
        "sha1" => Some(ChecksumAlgorithm::Sha1),
        "md5" => Some(ChecksumAlgorithm::Md5),
        _ => None,
    }
}

/// Hex digits a digest of `algorithm` is spelled with; `None` for one without a fixed width.
const fn hex_width(algorithm: ChecksumAlgorithm) -> Option<usize> {
    match algorithm {
        ChecksumAlgorithm::Sha256 => Some(64),
        ChecksumAlgorithm::Sha1 => Some(40),
        ChecksumAlgorithm::Md5 => Some(32),
        ChecksumAlgorithm::Crc32 | ChecksumAlgorithm::DropboxContentHash => None,
    }
}

fn checked_hex(algorithm: ChecksumAlgorithm, value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (Some(value.len()) == hex_width(algorithm)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then_some(value)
}

/// The strongest stated whole-file hash the host can check: SHA-256, then SHA-1, then MD5.
fn strongest(hashes: &[StatedHash]) -> Option<ExpectedChecksum> {
    let rank = |algorithm: ChecksumAlgorithm| match algorithm {
        ChecksumAlgorithm::Sha256 => 0,
        ChecksumAlgorithm::Sha1 => 1,
        _ => 2,
    };
    hashes
        .iter()
        .filter_map(|hash| {
            let algorithm = metalink_algorithm(&hash.algorithm)?;
            let value = checked_hex(algorithm, &hash.value)?;
            Some(ExpectedChecksum { algorithm, value })
        })
        .min_by_key(|checksum| rank(checksum.algorithm))
}

/// A piece list the host can verify against, or `None`.
///
/// Without a stated size the list cannot be matched to the file, and a list whose count does
/// not cover the size exactly describes some other file; both are dropped rather than
/// half-trusted.
fn checked_pieces(
    algorithm: &str,
    length: u64,
    hashes: Vec<String>,
    size: Option<u64>,
) -> Option<PieceHashes> {
    let algorithm = metalink_algorithm(algorithm)?;
    let size = size?;
    if !(MIN_PIECE_LENGTH..=MAX_PIECE_LENGTH).contains(&length)
        || hashes.is_empty()
        || hashes.len() > MAX_PIECES
        || size.div_ceil(length) != hashes.len() as u64
    {
        return None;
    }
    let hashes = hashes
        .iter()
        .map(|hash| checked_hex(algorithm, hash))
        .collect::<Option<Vec<_>>>()?;
    Some(PieceHashes {
        algorithm,
        length,
        hashes,
    })
}

/// One source of one queued download, with its health.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct DownloadSource {
    /// Place in the order, from zero; fixed when the download is created.
    pub position: u32,
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    pub protocol: SourceProtocol,
    pub priority: Option<u32>,
    pub location: Option<String>,
    /// Failures since the last delivery.
    pub failures: u32,
    /// Not tried again before this instant.
    pub backoff_until: Option<DateTime<Utc>>,
    /// Stable code of the reason this source is out for good, when it is.
    pub isolated_code: Option<String>,
    /// Stable code of the most recent failure.
    pub last_error_code: Option<String>,
    /// Bytes this source delivered that were confirmed.
    pub delivered_bytes: u64,
    /// Whether the set this source came from may reach the person's own network; see
    /// [`SourceSet::local_network`].
    #[serde(default)]
    pub local_network: bool,
}

/// What a source is, for the queue and the interface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Ready,
    BackingOff,
    Isolated,
    /// A protocol the chunk engine does not fetch from yet.
    Unsupported,
}

impl DownloadSource {
    /// Where this source stands at `now`.
    #[must_use]
    pub fn state_at(&self, now: DateTime<Utc>) -> SourceState {
        if self.isolated_code.is_some() {
            SourceState::Isolated
        } else if !self.protocol.serves_chunks() {
            SourceState::Unsupported
        } else if self.backoff_until.is_some_and(|until| until > now) {
            SourceState::BackingOff
        } else {
            SourceState::Ready
        }
    }
}

/// What one attempt learned about one source, as the writer records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceOutcome {
    /// Confirmed bytes arrived; the failure count starts over.
    Delivered { bytes: u64 },
    /// The source failed; it waits before it is tried again.
    Failed {
        code: String,
        retry_after_seconds: Option<u64>,
    },
    /// The source delivered wrong bytes or a different file; it is not tried again.
    Isolated { code: String },
}

/// How long a source that has now failed `failures` times waits: 30 s doubling to 30 min, or
/// what the server asked for when that is longer.
#[must_use]
pub fn source_backoff(failures: u32, retry_after_seconds: Option<u64>) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    let computed = BACKOFF_BASE_SECONDS
        .saturating_mul(1_i64 << exponent)
        .min(BACKOFF_MAX_SECONDS);
    // A Retry-After beyond i64 is still "wait as long as allowed", not "no wait at all".
    let asked = retry_after_seconds
        .map(|seconds| i64::try_from(seconds).unwrap_or(i64::MAX))
        .unwrap_or(0)
        .min(BACKOFF_MAX_SECONDS);
    Duration::seconds(computed.max(asked))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stated(algorithm: &str, value: &str) -> StatedHash {
        StatedHash {
            algorithm: algorithm.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn sources_are_ordered_by_priority_then_document_order() {
        let set = SourceSet::checked(
            [
                ("https://c.example/f".to_owned(), None, None),
                (
                    "https://b.example/f".to_owned(),
                    Some(2),
                    Some("DE".to_owned()),
                ),
                (
                    "https://a.example/f".to_owned(),
                    Some(1),
                    Some("usa".to_owned()),
                ),
                ("https://d.example/f".to_owned(), Some(2), None),
            ],
            None,
            &[],
            None,
        )
        .expect("set");
        let hosts: Vec<_> = set
            .sources
            .iter()
            .map(|source| source.url.host_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(hosts, ["a.example", "b.example", "d.example", "c.example"]);
        assert_eq!(set.sources[1].location.as_deref(), Some("de"));
        // Three letters is not an ISO 3166-1 alpha-2 code.
        assert_eq!(set.sources[0].location, None);
    }

    #[test]
    fn unusable_addresses_are_dropped_not_repaired() {
        let set = SourceSet::checked(
            [
                ("magnet:?xt=urn:btih:abc".to_owned(), None, None),
                ("file:///etc/passwd".to_owned(), None, None),
                ("ftp://user:secret@mirror.example/f".to_owned(), None, None),
                ("https://mirror.example/f".to_owned(), None, None),
                ("https://mirror.example/f".to_owned(), Some(1), None),
                ("sftp://mirror.example/f".to_owned(), None, None),
            ],
            None,
            &[],
            None,
        )
        .expect("set");
        assert_eq!(set.sources.len(), 2);
        assert_eq!(set.sources[0].url.as_str(), "https://mirror.example/f");
        assert_eq!(set.sources[1].url.scheme(), "sftp");
        assert!(
            SourceSet::checked(
                [("javascript:alert(1)".to_owned(), None, None)],
                None,
                &[],
                None
            )
            .is_none()
        );
    }

    #[test]
    fn the_strongest_valid_hash_wins_and_malformed_ones_are_ignored() {
        let sha256 = "a".repeat(64);
        let set = SourceSet::checked(
            [("https://m.example/f".to_owned(), None, None)],
            Some(10),
            &[
                stated("md5", &"b".repeat(32)),
                stated("sha-256", &"z".repeat(64)),
                stated("sha-256", &sha256.to_uppercase()),
                stated("sha-512", &"c".repeat(128)),
            ],
            None,
        )
        .expect("set");
        assert_eq!(
            set.checksum,
            Some(ExpectedChecksum {
                algorithm: ChecksumAlgorithm::Sha256,
                value: sha256,
            })
        );
        assert!(set.has_hash_basis());
    }

    #[test]
    fn a_piece_list_must_cover_the_stated_size_exactly() {
        let hash = "0".repeat(40);
        let source = || [("https://m.example/f".to_owned(), None, None)];
        let length = MIN_PIECE_LENGTH;
        let covering = SourceSet::checked(
            source(),
            Some(length * 2 + 1),
            &[],
            Some(("sha-1".to_owned(), length, vec![hash.clone(); 3])),
        )
        .expect("set");
        let pieces = covering.pieces.expect("pieces kept");
        assert_eq!(
            pieces.range(2, length * 2 + 1),
            (length * 2, length * 2 + 1)
        );
        // One hash short, no size, and a piece length below the floor are all refused.
        for (size, length, count) in [
            (Some(length * 2 + 1), length, 2),
            (None, length, 3),
            (Some(30), 10, 3),
        ] {
            let set = SourceSet::checked(
                source(),
                size,
                &[],
                Some(("sha-1".to_owned(), length, vec![hash.clone(); count])),
            )
            .expect("set");
            assert!(set.pieces.is_none());
        }
    }

    #[test]
    fn a_long_mirror_list_is_capped() {
        let many = (0..100).map(|index| (format!("https://m{index}.example/f"), None, None));
        let set = SourceSet::checked(many, None, &[], None).expect("set");
        assert_eq!(set.sources.len(), MAX_SOURCES);
    }

    #[test]
    fn oversized_piece_lists_addresses_and_foreign_schemes_are_refused() {
        let hash = "0".repeat(40);
        let source = || [("https://m.example/f".to_owned(), None, None)];
        // One piece more than the ceiling, covering its size exactly: refused by count alone.
        let pieces = MAX_PIECES + 1;
        let size = MIN_PIECE_LENGTH * pieces as u64;
        let set = SourceSet::checked(
            source(),
            Some(size),
            &[],
            Some((
                "sha-1".to_owned(),
                MIN_PIECE_LENGTH,
                vec![hash.clone(); pieces],
            )),
        )
        .expect("set");
        assert!(set.pieces.is_none());
        // A piece longer than the ceiling.
        let set = SourceSet::checked(
            source(),
            Some(MAX_PIECE_LENGTH + 1),
            &[],
            Some(("sha-1".to_owned(), MAX_PIECE_LENGTH + 1, vec![hash; 1])),
        )
        .expect("set");
        assert!(set.pieces.is_none());
        // An address longer than the ceiling, and schemes no runner speaks.
        let long = format!("https://m.example/{}", "a".repeat(MAX_SOURCE_URL));
        for address in [
            long.as_str(),
            "gopher://m.example/f",
            "dict://m.example:11211/stats",
            "ldap://m.example/o",
            "file://m.example/etc/passwd",
            "data:text/plain,hello",
        ] {
            assert!(
                SourceSet::checked([(address.to_owned(), None, None)], None, &[], None).is_none(),
                "{address}"
            );
        }
    }

    #[test]
    fn a_checked_set_stays_on_public_addresses_until_the_intake_says_otherwise() {
        let set = SourceSet::checked(
            [("http://192.168.1.10/f".to_owned(), None, None)],
            None,
            &[],
            None,
        )
        .expect("set");
        assert!(!set.local_network);
        // A set stored before the field existed reads as the strict one.
        let mut stored = serde_json::to_value(&set).expect("json");
        stored
            .as_object_mut()
            .expect("object")
            .remove("local_network");
        let read: SourceSet = serde_json::from_value(stored).expect("read");
        assert!(!read.local_network);
    }

    #[test]
    fn backoff_doubles_up_to_its_ceiling_and_honours_retry_after() {
        assert_eq!(source_backoff(1, None), Duration::seconds(30));
        assert_eq!(source_backoff(2, None), Duration::seconds(60));
        assert_eq!(source_backoff(40, None), Duration::seconds(1800));
        assert_eq!(source_backoff(1, Some(300)), Duration::seconds(300));
        assert_eq!(source_backoff(1, Some(u64::MAX)), Duration::seconds(1800));
    }

    #[test]
    fn state_follows_isolation_protocol_and_backoff() {
        let now = Utc::now();
        let mut source = DownloadSource {
            position: 0,
            url: Url::parse("https://m.example/f").expect("url"),
            protocol: SourceProtocol::Https,
            priority: None,
            location: None,
            failures: 0,
            backoff_until: None,
            isolated_code: None,
            last_error_code: None,
            delivered_bytes: 0,
            local_network: false,
        };
        assert_eq!(source.state_at(now), SourceState::Ready);
        source.backoff_until = Some(now + Duration::seconds(10));
        assert_eq!(source.state_at(now), SourceState::BackingOff);
        source.protocol = SourceProtocol::Ftp;
        assert_eq!(source.state_at(now), SourceState::Unsupported);
        source.isolated_code = Some(CODE_PIECE_MISMATCH.to_owned());
        assert_eq!(source.state_at(now), SourceState::Isolated);
    }
}

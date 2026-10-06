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
    /// Every protocol a source may carry: HTTP through the chunk engine's range requests, FTP
    /// (`REST`) and SFTP (a seek) through their runners, which open a connection at a chunk's
    /// offset for it.
    #[must_use]
    pub const fn serves_chunks(self) -> bool {
        matches!(
            self,
            Self::Http | Self::Https | Self::Ftp | Self::Ftps | Self::Sftp
        )
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

/// One source a LinkGrabber link carries, as the interface shows it before the link is queued
/// (RD-150-03). Redacted like every address the interface shows.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CandidateSource {
    /// The address, with credentials and signed query values replaced.
    pub url: String,
    pub host: Option<String>,
    pub protocol: SourceProtocol,
    /// Lower is preferred; absent when the document ranked it not at all.
    pub priority: Option<u32>,
    /// ISO 3166-1 alpha-2 country code the document gave.
    pub location: Option<String>,
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

    /// The one address of a link a document or a page proposed without naming mirrors, held
    /// to the same address rule a set is (RD-150-03).
    ///
    /// Written with the download, it is how the transfer knows the address is a stranger's:
    /// the queue fetches such a link on its single-source path, and holds it to the rule
    /// there. `None` for a scheme no source may carry. A password in the address is left out
    /// of the row, as for every source; the download keeps its own address.
    #[must_use]
    pub fn of_link(url: &Url, local_network: bool) -> Option<Self> {
        let mut address = url.clone();
        let _ = address.set_password(None);
        let url = checked_url(address.as_str())?;
        Some(Self {
            sources: vec![SetSource {
                url,
                priority: None,
                location: None,
            }],
            size: None,
            checksum: None,
            pieces: None,
            local_network,
        })
    }

    /// The sources as the LinkGrabber shows them, in the order the transfer will try them.
    #[must_use]
    pub fn preview(&self) -> Vec<CandidateSource> {
        self.sources
            .iter()
            .filter_map(|source| {
                Some(CandidateSource {
                    url: crate::redact_url(&source.url),
                    host: source.url.host_str().map(str::to_owned),
                    protocol: SourceProtocol::from_scheme(source.url.scheme())?,
                    priority: source.priority,
                    location: source.location.clone(),
                })
            })
            .collect()
    }

    /// Whether the set carries something that proves the bytes, which is the condition for
    /// mixing chunks from several sources into one file. Only the tests ask (DB-13).
    #[cfg(test)]
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
    /// A protocol the chunk engine does not fetch from. No protocol a source may carry is one
    /// since FTP and SFTP mirrors are fetched too; kept so the interface's contract holds.
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
    // The shared backoff (audit 1.9.1, INTAKE-12); any exponent past the old `min(16)` was
    // already beyond the cap, so the figures are unchanged.
    let computed = crate::timing::exponential_backoff(
        failures.saturating_sub(1),
        BACKOFF_BASE_SECONDS.unsigned_abs(),
        BACKOFF_MAX_SECONDS.unsigned_abs(),
    );
    let computed = i64::try_from(computed).unwrap_or(BACKOFF_MAX_SECONDS);
    // A Retry-After beyond i64 is still "wait as long as allowed", not "no wait at all".
    let asked = retry_after_seconds
        .map(|seconds| i64::try_from(seconds).unwrap_or(i64::MAX))
        .unwrap_or(0)
        .min(BACKOFF_MAX_SECONDS);
    Duration::seconds(computed.max(asked))
}

#[cfg(test)]
#[path = "source_set_tests.rs"]
mod tests;

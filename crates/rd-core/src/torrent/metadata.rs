//! Torrent metadata as rDownloader persists and exposes it: the file tree, the announce
//! list and the BEP 19 web seeds. Tracker URLs may carry a passkey, so everything that
//! leaves this crate towards the API is redacted by [`redact_tracker_url`].

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ByteCount;

/// Maximum number of trackers accepted on one torrent.
pub const MAX_TORRENT_TRACKERS: usize = 128;

/// Maximum accepted length of one tracker URL.
pub const MAX_TRACKER_URL: usize = 2048;

/// One file inside a torrent. `index` is the librqbit file index and stays stable for the
/// lifetime of the metadata, which is what makes it usable as a selection key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentFileEntry {
    pub index: u32,
    /// Path components relative to the torrent root, already sanitized.
    pub path: Vec<String>,
    pub length: ByteCount,
}

impl TorrentFileEntry {
    /// The path joined with `/`, as shown in the UI.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.path.join("/")
    }
}

/// Where a tracker entry came from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackerOrigin {
    /// Announce list of the torrent metadata.
    Metadata,
    /// Added by the user.
    User,
}

/// Cached scrape counters of one tracker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TrackerScrape {
    pub seeders: u32,
    pub leechers: u32,
    pub completed: u32,
    /// When the counters were fetched.
    pub scraped_at: chrono::DateTime<chrono::Utc>,
}

/// One tracker of a torrent. The URL is only ever exposed redacted; edits address the
/// entry through [`TorrentTracker::id`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TorrentTracker {
    /// Stable identifier derived from the URL, safe to expose.
    pub id: String,
    /// Full announce URL including any passkey. Never leaves the service unredacted.
    pub url: String,
    /// Announce tier; lower tiers are tried first.
    pub tier: u16,
    pub origin: TrackerOrigin,
    pub last_announce_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
    pub scrape: Option<TrackerScrape>,
}

impl TorrentTracker {
    /// Builds an entry with a derived id and no announce history yet.
    #[must_use]
    pub fn new(url: String, tier: u16, origin: TrackerOrigin) -> Self {
        Self {
            id: tracker_id(&url),
            url,
            tier,
            origin,
            last_announce_at: None,
            last_error: None,
            scrape: None,
        }
    }
}

/// Stable, non-secret identifier of a tracker URL.
///
/// A truncated FNV-1a digest: short enough for a URL path segment, and it never leaks the
/// passkey because the digest is one-way and the full URL never leaves the service.
#[must_use]
pub fn tracker_id(url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Replacement written in place of any credential material.
pub const TRACKER_REDACTION_PLACEHOLDER: &str = "[redacted]";

/// Query parameter names whose value is a credential on a tracker or web-seed URL.
///
/// Matched case-insensitively against the whole name, never as a substring, so a harmless
/// parameter is not swallowed because it happens to contain one of these words.
const TRACKER_SECRET_KEYS: &[&str] = &[
    "passkey",
    "authkey",
    "torrent_pass",
    "pid",
    "uk",
    "key",
    "secret",
    "token",
    "apikey",
    "api_key",
    "auth",
    "signature",
    "sig",
];

/// Removes credential material from a tracker or web-seed URL so it can be logged,
/// broadcast and returned by the API.
///
/// Three shapes are covered, which is every way a tracker credential actually travels:
/// userinfo (`user:pass@`), a passkey-style query parameter, and a passkey embedded as a
/// path segment — the convention most private trackers use.
#[must_use]
pub fn redact_tracker_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        // Not parseable, so nothing can be located reliably: never echo the input.
        return TRACKER_REDACTION_PLACEHOLDER.to_owned();
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        let _ = parsed.set_password(None);
        let _ = parsed.set_username("redacted");
    }
    let mut secrets_found = false;
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(name, value)| {
            if TRACKER_SECRET_KEYS.contains(&name.trim().to_ascii_lowercase().as_str()) {
                secrets_found = true;
                (name.into_owned(), TRACKER_REDACTION_PLACEHOLDER.to_owned())
            } else {
                (name.into_owned(), value.into_owned())
            }
        })
        .collect();
    if secrets_found {
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
    }
    if parsed
        .path_segments()
        .is_some_and(|mut segments| segments.any(is_secret_path_segment))
    {
        let masked: Vec<String> = parsed
            .path()
            .split('/')
            .map(|segment| {
                if is_secret_path_segment(segment) {
                    TRACKER_REDACTION_PLACEHOLDER.to_owned()
                } else {
                    segment.to_owned()
                }
            })
            .collect();
        parsed.set_path(&masked.join("/"));
    }
    parsed.into()
}

/// A path segment of at least 20 characters that is entirely alphanumeric and mixes in a
/// digit looks like an embedded passkey rather than a route such as `announce`.
fn is_secret_path_segment(segment: &str) -> bool {
    segment.len() >= 20
        && segment
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
        && segment.chars().any(|character| character.is_ascii_digit())
}

/// Everything rDownloader knows about one torrent's metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TorrentMetadataInfo {
    pub info_hash: String,
    pub name: String,
    pub total_bytes: ByteCount,
    pub piece_length: u64,
    pub piece_count: u32,
    /// Private torrents may not use DHT, PEX or LSD, and only announce to the first tracker.
    pub private: bool,
    pub files: Vec<TorrentFileEntry>,
    pub trackers: Vec<TorrentTracker>,
    /// BEP 19 `url-list` entries. The engine does not use them; they are diagnostics.
    pub web_seeds: Vec<String>,
}

impl TorrentMetadataInfo {
    /// Sum of the lengths of the given file indices.
    #[must_use]
    pub fn selected_bytes(&self, selected: &std::collections::BTreeSet<u32>) -> u64 {
        self.files
            .iter()
            .filter(|file| selected.contains(&file.index))
            .map(|file| file.length.get())
            .sum()
    }

    /// Whether every index exists in this metadata.
    #[must_use]
    pub fn contains_all(&self, indices: impl IntoIterator<Item = u32>) -> bool {
        let known: std::collections::BTreeSet<u32> =
            self.files.iter().map(|file| file.index).collect();
        indices.into_iter().all(|index| known.contains(&index))
    }
}

#[cfg(test)]
mod tests {
    use super::{TorrentTracker, TrackerOrigin, redact_tracker_url, tracker_id};

    #[test]
    fn userinfo_is_removed_from_tracker_urls() {
        let redacted = redact_tracker_url("http://user:secret@tracker.example/announce");
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("user"));
        assert!(redacted.contains("tracker.example"));
    }

    #[test]
    fn secret_query_values_are_masked() {
        let redacted =
            redact_tracker_url("https://tracker.example/announce?passkey=abc123&info_hash=x");
        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("passkey="));
        assert!(redacted.contains("info_hash=x"));
    }

    #[test]
    fn hex_path_segments_are_masked_but_routes_survive() {
        let redacted =
            redact_tracker_url("https://tracker.example/0123456789abcdef0123456789/announce");
        assert!(redacted.contains(super::TRACKER_REDACTION_PLACEHOLDER));
        assert!(redacted.ends_with("/announce"));
        assert!(!redacted.contains("0123456789abcdef"));
    }

    #[test]
    fn unparseable_urls_never_echo_their_content() {
        assert_eq!(
            redact_tracker_url("not a url at all"),
            super::TRACKER_REDACTION_PLACEHOLDER
        );
    }

    #[test]
    fn tracker_ids_are_stable_and_hide_the_url() {
        let url = "https://tracker.example/announce?passkey=abc123";
        let tracker = TorrentTracker::new(url.to_owned(), 0, TrackerOrigin::Metadata);
        assert_eq!(tracker.id, tracker_id(url));
        assert!(!tracker.id.contains("abc123"));
        assert_ne!(tracker.id, tracker_id("https://other.example/announce"));
    }
}

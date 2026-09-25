use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use url::Url;
use utoipa::ToSchema;

use crate::{
    AccountId, CategoryId, DownloadId, ExtractionResult, MAX_PERSISTED_BYTES, NzbFileId,
    NzbImportId, PackageId, PackageState, PostprocessLevel, PostprocessStatus, ProxyProfileId,
};

/// Byte count serialized as a decimal string for JavaScript safety.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, ToSchema)]
#[schema(value_type = String, example = "4294967296")]
pub struct ByteCount(u64);

impl ByteCount {
    /// Creates a byte count accepted by SQLite persistence.
    pub fn new(value: u64) -> Result<Self, &'static str> {
        if value > MAX_PERSISTED_BYTES {
            return Err("byte count exceeds SQLite INTEGER range");
        }
        Ok(Self(value))
    }

    /// Returns the raw value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for ByteCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ByteCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let parsed = value.parse::<u64>().map_err(de::Error::custom)?;
        Self::new(parsed).map_err(de::Error::custom)
    }
}

/// Persistent lifecycle of a download.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Resolving,
    Downloading,
    Paused,
    RetryWait,
    Verifying,
    Repairing,
    Extracting,
    /// Torrent payload is complete and being uploaded to peers; ends in `Completed`.
    Seeding,
    /// Held back because another link to the same file is the one being downloaded.
    ///
    /// Not a failure and not a terminal state: if the active mirror gives up, one of these
    /// takes over. A person can also start it by hand, which skips the others instead.
    Skipped,
    Blocked,
    Failed,
    Cancelled,
    Completed,
}

impl DownloadState {
    /// Returns whether this state can move to `next` without repairing history.
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        use DownloadState::{
            Blocked, Cancelled, Completed, Downloading, Extracting, Failed, Paused, Queued,
            Repairing, Resolving, RetryWait, Seeding, Skipped, Verifying,
        };

        matches!(
            (self, next),
            // `Blocked` from `Queued` is what switching off a download kind does: the entry
            // never started, so there is nothing to stop, but it must not be dispatched
            // either. Its absence made `block_queued_of_kind` fail every single time it ran
            // — it logged a warning and left the row `Queued`, so a disabled kind kept being
            // picked up by the next pass.
            (Queued, Resolving | Paused | Blocked | Cancelled)
                | (
                    Resolving,
                    Downloading | RetryWait | Blocked | Failed | Cancelled
                )
                | (
                    Downloading,
                    Paused | RetryWait | Verifying | Seeding | Blocked | Failed | Cancelled
                )
                | (Seeding, Completed | Failed | Cancelled)
                | (Paused, Queued | Downloading | Cancelled)
                | (
                    RetryWait,
                    Queued | Resolving | Downloading | Paused | Failed | Cancelled
                )
                | (Verifying, Repairing | Extracting | Completed | Failed)
                | (Repairing, Extracting | Completed | Failed)
                | (Extracting, Completed | Failed)
                | (Completed, Extracting)
                | (Blocked, Queued | Resolving | Cancelled)
                | (Failed, Queued | Cancelled)
                | (Cancelled, Queued)
                // A mirror is stood down from anything that has not started, and from a
                // failure, so a package that gave up can still be retried through another
                // link. It goes back to the queue when the active mirror gives up.
                | (
                    Queued | Paused | RetryWait | Failed | Blocked,
                    Skipped
                )
                | (Skipped, Queued | Cancelled)
        ) || self == next
    }
}

impl fmt::Display for DownloadState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let serialized = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        formatter.write_str(serialized.trim_matches('"'))
    }
}

impl FromStr for DownloadState {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{value}\""))
    }
}

/// Supported checksum algorithms.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Crc32,
    /// Dropbox's `content_hash` (RD-106-06): SHA-256 over each 4 MiB block of the file, then
    /// SHA-256 over the concatenated block digests. Not a plain digest of the bytes, so it
    /// needs a name of its own — stated as `sha256` it would fail every file it was meant to
    /// verify.
    DropboxContentHash,
}

/// Checksum expected from metadata or the user.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ExpectedChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub value: String,
}

/// Queue priority of a package; higher priorities are scheduled first.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPriority {
    Low,
    #[default]
    Normal,
    High,
}

impl DownloadPriority {
    /// Persistent integer representation (`-1`, `0`, `1`).
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Low => -1,
            Self::Normal => 0,
            Self::High => 1,
        }
    }

    /// Maps any stored integer back; unknown values become `Normal`.
    #[must_use]
    pub const fn from_i32(value: i32) -> Self {
        match value {
            i32::MIN..=-1 => Self::Low,
            0 => Self::Normal,
            _ => Self::High,
        }
    }
}

/// Transport family of a package/file: direct HTTP (incl. hoster resolvers), Usenet or
/// media fetched through an external extractor (yt-dlp).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadKind {
    #[default]
    Http,
    Usenet,
    Media,
    /// Image gallery fetched by gallery-dl; one row covers the whole gallery.
    Gallery,
    /// Livestream recording via streamlink; open-ended until stopped or the stream ends.
    Record,
    /// BitTorrent transfer via the embedded engine; one row covers the whole torrent.
    Torrent,
    /// FTP or FTPS transfer via the native client; one row per remote file.
    Ftp,
    /// SFTP transfer over SSH; one row per remote file.
    Sftp,
    /// A protocol carried by an installed transfer backend; one row per remote file.
    Plugin,
}

/// A logical package grouping one or more files.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DownloadPackage {
    pub id: PackageId,
    pub name: String,
    #[serde(default)]
    pub state: PackageState,
    pub created_at: DateTime<Utc>,
    pub destination: String,
    pub category_id: Option<CategoryId>,
    pub priority: DownloadPriority,
    /// Manual queue position inside the same priority (lower runs first).
    pub position: i64,
    /// Whether an archive password is stored.
    #[serde(default)]
    pub has_password: bool,
    /// The stored archive password, in clear.
    ///
    /// Deliberately readable (RD-104-04): an archive password comes from the release title,
    /// the `{{password}}` marker of a file name or the feed, so it is public already, and a
    /// person who wants to unpack by hand — or to understand why an unpack failed — needs it.
    /// This is the *only* secret rDownloader hands back: account passwords, API keys and NNTP
    /// or proxy credentials live in `rd-secrets` and stay write-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub kind: DownloadKind,
    /// Backing NZB import for Usenet packages.
    pub nzb_import_id: Option<NzbImportId>,
    /// When the package reached `Completed`, for the automatic removal of finished packages.
    ///
    /// Distinct from the row's last write: editing a finished package must not look like it
    /// finished again.
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    /// Explicit post-processing level; `None` inherits category/default.
    #[serde(default)]
    pub postprocess_level: Option<PostprocessLevel>,
    /// Post-processing script file name (inside the scripts directory); `None` inherits.
    #[serde(default)]
    pub script: Option<String>,
    /// Live stage/progress while `state == postprocessing`.
    #[serde(default)]
    pub postprocess: PostprocessStatus,
    /// Outcome of the unpack stage of the last post-processing run; `None` when nothing
    /// was extracted.
    #[serde(default)]
    pub extraction_result: Option<ExtractionResult>,
    /// Fields enrichers contributed to the links this package was built from (RD-107-02).
    ///
    /// The union of its files' fields, carried across the enqueue. Without it a rating an
    /// enricher looked up was visible only while the link sat in the LinkGrabber, which for
    /// an auto-queueing subscription is a few seconds.
    #[serde(default)]
    pub enrichment: Vec<crate::EnrichmentField>,
}

/// One downloadable file within a package.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DownloadFile {
    pub id: DownloadId,
    pub package_id: PackageId,
    #[schema(value_type = String, format = "uri")]
    pub source: Url,
    pub file_name: String,
    pub state: DownloadState,
    pub total_bytes: Option<ByteCount>,
    pub committed_bytes: ByteCount,
    pub retry_count: u32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub expected_checksum: Option<ExpectedChecksum>,
    pub computed_checksum: Option<ExpectedChecksum>,
    pub last_error: Option<crate::Failure>,
    pub account_id: Option<AccountId>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    /// Stored login for `kind == ftp | sftp`; `None` matches the URL against the remote
    /// credential store by host and port when the transfer starts.
    #[serde(default)]
    pub remote_credential_id: Option<crate::RemoteCredentialId>,
    /// Key shared by links in the same package that point at the same file.
    ///
    /// Only one member of a group downloads; the rest wait as `Skipped` until it either
    /// finishes, which leaves them where they are, or gives up, which promotes one of them.
    #[serde(default)]
    pub mirror_group: Option<String>,
    /// Which auth profile this job uses: auto-match by scope, none, or a pinned one.
    #[serde(default)]
    pub auth_profile: crate::AuthProfileSelection,
    /// Segment history and sidecars of a livestream recording (RD-080-09); `None` for every
    /// other kind of download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::RecordingState>,
    /// Order inside the package (lower first).
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub kind: DownloadKind,
    /// Backing NZB file for Usenet downloads.
    pub nzb_file_id: Option<NzbFileId>,
    /// PAR2 repair data rather than payload, decided when the NZB is queued.
    ///
    /// A recovery volume that never arrives is not automatically a defect: as long as the
    /// payload is complete, nobody asked for it. Until RD-107-10 the difference existed only
    /// on disk, in post-processing, so the queue had to treat every lost volume as a failure.
    #[serde(default)]
    pub recovery: bool,
    /// Selected media variant for `kind == media`.
    #[serde(default)]
    pub media: Option<crate::MediaSelection>,
    /// Fields enrichers contributed to the link this row was queued from (RD-107-02).
    ///
    /// Beside the core fields, never merged into them — the same rule the candidate column
    /// follows, so what a plugin said stays distinguishable from what the core resolved.
    #[serde(default)]
    pub enrichment: Vec<crate::EnrichmentField>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Whether a queued file is PAR2 repair data rather than payload.
///
/// Name-based on purpose: the NZB says nothing else about a file before its first segment
/// arrives, and it is the same rule post-processing applies to the finished directory. Both
/// the main `.par2` index and the `vol###+##.par2` volumes count as recovery.
///
/// Built from the two rules that decide postponement (RD-107-04) rather than restating them:
/// a second definition of "is this PAR2" is a second definition that can drift.
#[must_use]
pub fn is_recovery_volume(file_name: &str) -> bool {
    crate::postprocess::is_par2_index(file_name) || crate::postprocess::is_par2_volume(file_name)
}

#[cfg(test)]
mod tests {
    use super::{ByteCount, DownloadState};

    #[test]
    fn bytes_are_json_strings() {
        let serialized = serde_json::to_string(&ByteCount::new(4_294_967_296).expect("valid"));
        assert!(matches!(serialized.as_deref(), Ok("\"4294967296\"")));
    }

    #[test]
    fn recognizes_recovery_volumes_by_name() {
        assert!(super::is_recovery_volume("Release.par2"));
        assert!(super::is_recovery_volume("Release.vol012+10.PAR2"));
        assert!(!super::is_recovery_volume("Release.part01.rar"));
        assert!(!super::is_recovery_volume("par2"));
    }

    #[test]
    fn rejects_invalid_transition() {
        assert!(!DownloadState::Completed.can_transition_to(DownloadState::Downloading));
        assert!(DownloadState::Downloading.can_transition_to(DownloadState::Paused));
        assert!(DownloadState::Cancelled.can_transition_to(DownloadState::Queued));
    }

    /// Switching off a download kind has to reach the entries that never started.
    ///
    /// This edge was missing, so `block_queued_of_kind` failed on every row it touched and
    /// only logged a warning — a disabled kind went on being dispatched.
    #[test]
    fn a_queued_download_can_be_blocked() {
        assert!(DownloadState::Queued.can_transition_to(DownloadState::Blocked));
        assert!(DownloadState::Blocked.can_transition_to(DownloadState::Queued));
    }
}

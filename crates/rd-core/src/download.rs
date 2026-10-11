use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{
    AccountId, ByteCount, CategoryId, ChecksumAlgorithm, DownloadId, ExtractionResult, NzbFileId,
    NzbImportId, PackageId, PackageState, PostprocessLevel, PostprocessStatus, ProxyProfileId,
};

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
                // `Paused` from `Resolving` is a pause that reaches the worker before the source
                // answered. Without it the worker's stop was refused, the refusal was recorded
                // as a failed attempt, and the file went to `RetryWait` and started again
                // (RD-1130-04).
                | (
                    Resolving,
                    Downloading | Paused | RetryWait | Blocked | Failed | Cancelled
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
                // `Blocked` from `Verifying` is the `ask` collision policy meeting a name that was
                // taken while the transfer ran (RD-150-01): the verified file waits in staging
                // for an answer, and only this download waits with it. `Cancelled` is for a row
                // that waits for its set's PAR2 verdict with no worker behind it (RD-108-24);
                // without it such a row could be neither cancelled nor removed.
                | (
                    Verifying,
                    Repairing | Extracting | Completed | Failed | Blocked | Cancelled
                )
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

    /// A worker runs the entry right now: resolving, fetching, verifying, repairing or
    /// unpacking. Seeding is not counted — it is stopped through the torrent engine.
    #[must_use]
    pub const fn is_working(self) -> bool {
        matches!(
            self,
            Self::Resolving
                | Self::Downloading
                | Self::Verifying
                | Self::Repairing
                | Self::Extracting
        )
    }

    /// The payload is being written, read or served: [`Self::is_working`] or seeding.
    #[must_use]
    pub const fn holds_the_file(self) -> bool {
        self.is_working() || matches!(self, Self::Seeding)
    }

    /// Waiting for its turn (`Queued`, `RetryWait`) or [`Self::is_working`] — what a pause acts
    /// on and what has to be stopped before the entry may be removed.
    #[must_use]
    pub const fn is_queued_or_working(self) -> bool {
        self.is_working() || matches!(self, Self::Queued | Self::RetryWait)
    }

    /// A runner holds a network connection: resolving, downloading or seeding.
    #[must_use]
    pub const fn holds_a_connection(self) -> bool {
        matches!(self, Self::Resolving | Self::Downloading | Self::Seeding)
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
    /// An object in an S3-compatible bucket (RD-150-04); one row per object.
    ObjectStorage,
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
    /// The package's files start no earlier than this (RD-1240-14); `None` starts them as the
    /// queue reaches them. A moment that has passed holds nothing back and stays until the next
    /// edit.
    #[serde(default)]
    pub start_after: Option<DateTime<Utc>>,
    /// The package's own download window (RD-1240-30); `None` follows its category's, and a
    /// category without one leaves the package to the bandwidth schedule alone.
    #[serde(default)]
    pub download_window: Option<crate::DownloadWindow>,
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
    use super::DownloadState;

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

    /// A file paused while its source has not answered yet stops as paused (RD-1130-04).
    #[test]
    fn a_resolving_download_can_be_paused() {
        assert!(DownloadState::Resolving.can_transition_to(DownloadState::Paused));
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

    /// A verified file whose name was taken during the transfer waits for an `ask` answer in
    /// `Blocked`, and the answer puts it back into the queue (RD-150-01).
    #[test]
    fn a_verified_download_can_wait_for_a_collision_answer() {
        assert!(DownloadState::Verifying.can_transition_to(DownloadState::Blocked));
        assert!(!DownloadState::Completed.can_transition_to(DownloadState::Blocked));
    }
}

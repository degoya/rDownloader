//! Directory listings of a remote server, reviewed in the LinkGrabber before anything is
//! queued.
//!
//! The shape follows the torrent file plan ([`crate::ResolvedTorrentPlan`]): a listing is
//! stored on the link candidate, the user reviews a tree, and only the selected entries
//! become queue rows. Entries are addressed by their path rather than by an index, because
//! a re-listing of a live directory does not preserve ordering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ByteCount;

/// Most entries kept from one listing. A directory with more is reported as truncated
/// rather than silently cut, so nobody queues "everything" and gets a prefix.
pub const MAX_REMOTE_ENTRIES: usize = 5_000;
/// Deepest directory nesting walked during recursive expansion.
pub const MAX_REMOTE_DEPTH: usize = 16;
/// Longest single remote path accepted.
pub const MAX_REMOTE_PATH: usize = 4_096;

/// One file or directory returned by a remote listing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RemoteEntry {
    /// Path relative to the listing root, using `/` separators and never starting with one.
    pub path: String,
    pub is_dir: bool,
    pub size: Option<ByteCount>,
    pub modified: Option<DateTime<Utc>>,
    /// Server-supplied validator (WebDAV `ETag`); `None` for FTP and SFTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

impl RemoteEntry {
    /// Last path segment.
    #[must_use]
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Why a listing stopped early.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListingLimit {
    EntryCount,
    Depth,
}

/// What one remote link resolved to.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(default)]
pub struct RemoteListing {
    /// Absolute path on the server the entries are relative to.
    pub root: String,
    /// `true` when the link addressed a single file rather than a directory.
    pub single_file: bool,
    pub entries: Vec<RemoteEntry>,
    /// Set when a limit stopped the walk; the entries present are still valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<ListingLimit>,
    /// Whether the server offered a resumable transfer (FTP `REST`, HTTP `Accept-Ranges`).
    /// `false` means an interrupted download has to start over, which the UI states.
    #[serde(default)]
    pub supports_resume: bool,
}

impl RemoteListing {
    /// Bounded form carried in list responses, so a directory with thousands of files does
    /// not inflate every candidate poll.
    #[must_use]
    pub fn summary(&self) -> RemoteListingSummary {
        let files = self.entries.iter().filter(|entry| !entry.is_dir);
        RemoteListingSummary {
            root: self.root.clone(),
            single_file: self.single_file,
            file_count: files.clone().count(),
            total_bytes: sum_bytes(files),
            truncated: self.truncated,
            supports_resume: self.supports_resume,
        }
    }
}

/// Listing metadata without the entries themselves.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RemoteListingSummary {
    pub root: String,
    pub single_file: bool,
    pub file_count: usize,
    pub total_bytes: ByteCount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<ListingLimit>,
    #[serde(default)]
    pub supports_resume: bool,
}

/// The user's selection: everything is included unless its path (or a parent directory)
/// was excluded. Storing exclusions rather than inclusions keeps a re-listing that adds a
/// new file from silently dropping it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RemoteListingPlan {
    #[serde(default)]
    pub excluded: Vec<String>,
}

/// One entry with the selection applied.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ResolvedRemoteEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: Option<ByteCount>,
    pub modified: Option<DateTime<Utc>>,
    pub included: bool,
}

/// A listing plus its selection, as handed to the UI and used at enqueue time.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ResolvedRemoteListing {
    pub root: String,
    pub single_file: bool,
    pub entries: Vec<ResolvedRemoteEntry>,
    pub selected_files: usize,
    pub selected_bytes: ByteCount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<ListingLimit>,
    #[serde(default)]
    pub supports_resume: bool,
}

/// Version of the persisted remote listing blob.
pub const REMOTE_CONTRACT_VERSION: u32 = 1;

/// What is stored next to a link candidate for an ftp/sftp/webdav link.
///
/// A typed JSON blob following the `torrent_json` precedent: it carries a
/// [`REMOTE_CONTRACT_VERSION`] so a future format change is detectable instead of being
/// silently misread.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RemoteCandidateState {
    pub contract_version: u32,
    pub listing: RemoteListing,
    pub plan: RemoteListingPlan,
}

impl Default for RemoteCandidateState {
    fn default() -> Self {
        Self {
            contract_version: REMOTE_CONTRACT_VERSION,
            listing: RemoteListing::default(),
            plan: RemoteListingPlan::default(),
        }
    }
}

impl RemoteCandidateState {
    /// Candidate state for a freshly probed link.
    #[must_use]
    pub fn resolved(listing: RemoteListing) -> Self {
        Self {
            listing,
            ..Self::default()
        }
    }

    /// Whether the blob was written by a newer version of rDownloader.
    #[must_use]
    pub const fn is_future_contract(&self) -> bool {
        self.contract_version > REMOTE_CONTRACT_VERSION
    }

    /// The selection applied to the listing.
    #[must_use]
    pub fn resolve(&self) -> ResolvedRemoteListing {
        resolve_listing(&self.listing, &self.plan)
    }

    /// The list-sized view of this state.
    #[must_use]
    pub fn summary(&self) -> RemoteListingSummary {
        let resolved = self.resolve();
        RemoteListingSummary {
            root: self.listing.root.clone(),
            single_file: self.listing.single_file,
            file_count: resolved.selected_files,
            total_bytes: resolved.selected_bytes,
            truncated: self.listing.truncated,
            supports_resume: self.listing.supports_resume,
        }
    }
}

/// Applies `plan` to `listing`.
///
/// Excluding a directory excludes everything below it, which is what a folder checkbox in
/// the tree means. Matching is on segment boundaries so excluding `extras` never also
/// drops `extras-2`.
#[must_use]
pub fn resolve_listing(listing: &RemoteListing, plan: &RemoteListingPlan) -> ResolvedRemoteListing {
    let entries: Vec<ResolvedRemoteEntry> = listing
        .entries
        .iter()
        .map(|entry| ResolvedRemoteEntry {
            included: !is_excluded(&entry.path, &plan.excluded),
            path: entry.path.clone(),
            is_dir: entry.is_dir,
            size: entry.size,
            modified: entry.modified,
        })
        .collect();
    let selected = entries
        .iter()
        .filter(|entry| entry.included && !entry.is_dir);
    ResolvedRemoteListing {
        root: listing.root.clone(),
        single_file: listing.single_file,
        selected_files: selected.clone().count(),
        selected_bytes: sum_bytes_resolved(selected),
        truncated: listing.truncated,
        supports_resume: listing.supports_resume,
        entries,
    }
}

/// Whether `path` is itself excluded or lies inside an excluded directory.
fn is_excluded(path: &str, excluded: &[String]) -> bool {
    excluded.iter().any(|prefix| {
        let prefix = prefix.trim_end_matches('/');
        if prefix.is_empty() {
            return false;
        }
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// Whether a server-supplied path is safe to join onto a local destination.
///
/// Refuses absolute paths, Windows drive letters, UNC prefixes, `..` segments and NUL, so a
/// hostile or broken listing cannot write outside the package folder. This runs *before*
/// [`crate::MAX_PERSISTED_BYTES`]-style storage checks, not instead of them: the local join
/// is still confined by `StorageRoot::resolve`.
#[must_use]
pub fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_REMOTE_PATH || path.contains('\0') {
        return false;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    // `C:\share` and `\\host\share` are absolute on Windows even though they have no
    // leading slash in the POSIX sense.
    if path.chars().nth(1) == Some(':') {
        return false;
    }
    path.split(['/', '\\'])
        .all(|segment| !matches!(segment, ".." | "." | ""))
}

fn sum_bytes<'a>(entries: impl Iterator<Item = &'a RemoteEntry>) -> ByteCount {
    let total = entries
        .filter_map(|entry| entry.size)
        .fold(0u64, |acc, size| acc.saturating_add(size.get()));
    ByteCount::new(total.min(crate::MAX_PERSISTED_BYTES)).unwrap_or_default()
}

fn sum_bytes_resolved<'a>(entries: impl Iterator<Item = &'a ResolvedRemoteEntry>) -> ByteCount {
    let total = entries
        .filter_map(|entry| entry.size)
        .fold(0u64, |acc, size| acc.saturating_add(size.get()));
    ByteCount::new(total.min(crate::MAX_PERSISTED_BYTES)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteEntry, RemoteListing, RemoteListingPlan, is_safe_relative_path, resolve_listing,
    };
    use crate::ByteCount;

    fn entry(path: &str, is_dir: bool, size: u64) -> RemoteEntry {
        RemoteEntry {
            path: path.to_owned(),
            is_dir,
            size: (!is_dir).then(|| ByteCount::new(size).expect("size")),
            modified: None,
            etag: None,
        }
    }

    fn listing() -> RemoteListing {
        RemoteListing {
            root: "/pub".to_owned(),
            single_file: false,
            entries: vec![
                entry("extras", true, 0),
                entry("extras/notes.txt", false, 10),
                entry("extras-2", true, 0),
                entry("extras-2/keep.bin", false, 20),
                entry("movie.mkv", false, 100),
            ],
            truncated: None,
            supports_resume: true,
        }
    }

    #[test]
    fn everything_is_selected_without_a_plan() {
        let resolved = resolve_listing(&listing(), &RemoteListingPlan::default());
        assert_eq!(resolved.selected_files, 3);
        assert_eq!(resolved.selected_bytes.get(), 130);
    }

    #[test]
    fn excluding_a_folder_excludes_its_children_only() {
        let plan = RemoteListingPlan {
            excluded: vec!["extras".to_owned()],
        };
        let resolved = resolve_listing(&listing(), &plan);
        // The bug this locks in: a plain `starts_with` would also drop `extras-2/keep.bin`.
        let kept: Vec<&str> = resolved
            .entries
            .iter()
            .filter(|entry| entry.included && !entry.is_dir)
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(kept, ["extras-2/keep.bin", "movie.mkv"]);
        assert_eq!(resolved.selected_bytes.get(), 120);
    }

    #[test]
    fn a_single_file_exclusion_leaves_its_siblings() {
        let plan = RemoteListingPlan {
            excluded: vec!["movie.mkv".to_owned()],
        };
        let resolved = resolve_listing(&listing(), &plan);
        assert_eq!(resolved.selected_files, 2);
        assert_eq!(resolved.selected_bytes.get(), 30);
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        assert!(is_safe_relative_path("dir/file.bin"));
        assert!(is_safe_relative_path("a b/c.bin"));
        for hostile in [
            "../escape",
            "dir/../../escape",
            "/etc/passwd",
            "\\\\host\\share",
            "C:\\Windows",
            "dir\\..\\escape",
            "with\0nul",
            "",
        ] {
            assert!(!is_safe_relative_path(hostile), "{hostile}");
        }
    }

    #[test]
    fn summary_counts_files_not_directories() {
        let summary = listing().summary();
        assert_eq!(summary.file_count, 3);
        assert_eq!(summary.total_bytes.get(), 130);
        assert!(summary.supports_resume);
    }
}

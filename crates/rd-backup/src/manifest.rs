//! What an archive says about itself (RD-160-01): format version 1.
//!
//! The manifest is the first member of the archive's tar stream and names every other member
//! with its size and SHA-256. A restore (RD-160-03) and the integrity check of RD-160-02 read it
//! before anything else, and a member it does not name is refused rather than unpacked.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The format name every manifest carries.
pub const FORMAT: &str = "rdownloader-backup";
/// The format version this build writes and reads.
pub const FORMAT_VERSION: u32 = 1;
/// The manifest's own member name; always the first member.
pub const MANIFEST_NAME: &str = "manifest.json";
/// The largest manifest a reader accepts.
pub const MAX_MANIFEST_BYTES: u64 = 16 << 20;

/// Member names of the fixed parts.
pub const SETTINGS_PART: &str = "settings.json";
pub const DATABASE_PART: &str = "database.sqlite3";
pub const PLUGIN_TRUST_PART: &str = "plugin-trust.json";
pub const PARTIAL_TRANSFERS_PART: &str = "partial-transfers.json";
/// Folder prefixes of the parts copied file by file.
pub const TORRENT_SESSION_PREFIX: &str = "torrent-session";
pub const TORRENT_FILES_PREFIX: &str = "torrents";

/// What a part is.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// The settings bundle, its credentials sealed under the backup key.
    Settings,
    /// The consistent copy of the database: queue, LinkGrabber, post-processing, history.
    Database,
    /// The plugin trust tables, read from the copy.
    PluginTrust,
    /// Every unfinished download with its checkpoints, read from the copy.
    PartialTransfers,
    /// One file of the torrent engine's persisted session.
    TorrentSession,
    /// One stored `.torrent` file a queue row points at.
    TorrentFile,
}

/// One member of the archive.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManifestPart {
    /// The member name, `/`-separated.
    pub name: String,
    pub kind: PartKind,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// The first member of every archive.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    pub created_at: DateTime<Utc>,
    /// The version of rDownloader that wrote the archive.
    pub app_version: String,
    pub parts: Vec<ManifestPart>,
}

impl Manifest {
    /// A manifest of this format version.
    #[must_use]
    pub fn new(created_at: DateTime<Utc>, app_version: String, parts: Vec<ManifestPart>) -> Self {
        Self {
            format: FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            created_at,
            app_version,
            parts,
        }
    }

    /// Whether this build can read what the manifest describes.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.format == FORMAT && self.format_version == FORMAT_VERSION
    }

    /// The part of a member name, if the manifest names it.
    #[must_use]
    pub fn part(&self, name: &str) -> Option<&ManifestPart> {
        self.parts.iter().find(|part| part.name == name)
    }
}

/// Whether `name` is a member name an archive may carry: relative, `/`-separated, no empty,
/// `.` or `..` segment, no backslash or drive colon. Checked on writing and on reading, so a
/// crafted archive cannot place a file outside the folder it is opened into.
#[must_use]
pub fn is_safe_member_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1_024
        && !name.contains(['\\', ':', '\0'])
        && name
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

#[cfg(test)]
mod tests {
    use super::is_safe_member_name;

    #[test]
    fn only_plain_relative_member_names_are_accepted() {
        for good in [
            "manifest.json",
            "torrent-session/session.json",
            "torrents/a b.torrent",
        ] {
            assert!(is_safe_member_name(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../outside",
            "torrents/../../outside",
            "torrents//double",
            "./here",
            "C:/Windows",
            "torrents\\evil",
            "trailing/",
        ] {
            assert!(!is_safe_member_name(bad), "{bad}");
        }
    }
}

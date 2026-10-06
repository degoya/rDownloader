//! Turning a remote SFTP directory into the reviewable [`RemoteListing`].
//!
//! SFTP reports file type and size in a defined structure, so unlike FTP there is no
//! listing dialect to guess at; the only judgement calls are how far to walk and which
//! entries are safe to store locally.

use chrono::{DateTime, Utc};
use rd_core::{ByteCount, RemoteEntry, RemoteListing};
use rd_transfer_file::{DirectoryLister, ListedEntry};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;

/// Walks `root` recursively and returns everything below it, through the walk SFTP shares with
/// FTP (`rd_transfer_file::walk`): bounded on entry count and depth, breadth first.
pub(crate) async fn walk(sftp: &SftpSession, root: &str) -> anyhow::Result<RemoteListing> {
    let (entries, truncated) = rd_transfer_file::walk(&mut SftpLister(sftp), root, "sftp").await?;
    Ok(RemoteListing {
        root: root.to_owned(),
        single_file: false,
        entries,
        // SFTP reads from an explicit offset, so resume is always available.
        supports_resume: true,
        truncated,
    })
}

/// One SFTP directory read.
struct SftpLister<'a>(&'a SftpSession);

impl DirectoryLister for SftpLister<'_> {
    async fn list(&mut self, absolute: &str) -> anyhow::Result<Option<Vec<ListedEntry>>> {
        let listed = match self.0.read_dir(absolute.to_owned()).await {
            Ok(listed) => listed,
            // A directory that became unreadable mid-walk should not lose the entries
            // already collected from its siblings.
            Err(error) => {
                tracing::warn!(%error, "skipping unreadable SFTP directory");
                return Ok(None);
            }
        };
        Ok(Some(
            listed
                .into_iter()
                .map(|entry| {
                    let metadata = entry.metadata();
                    let is_dir = metadata.is_dir();
                    ListedEntry {
                        name: entry.file_name(),
                        entry: RemoteEntry {
                            path: String::new(),
                            is_dir,
                            size: (!is_dir).then(|| size_of(&metadata)).flatten(),
                            modified: modified_at(&metadata),
                            etag: None,
                        },
                        descend: is_dir && !metadata.is_symlink(),
                    }
                })
                .collect(),
        ))
    }
}

pub(crate) fn size_of(metadata: &FileAttributes) -> Option<ByteCount> {
    metadata.size.and_then(|size| ByteCount::new(size).ok())
}

pub(crate) fn modified_at(metadata: &FileAttributes) -> Option<DateTime<Utc>> {
    metadata.modified().ok().map(DateTime::<Utc>::from)
}

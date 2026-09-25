//! Turning a remote SFTP directory into the reviewable [`RemoteListing`].
//!
//! SFTP reports file type and size in a defined structure, so unlike FTP there is no
//! listing dialect to guess at; the only judgement calls are how far to walk and which
//! entries are safe to store locally.

use chrono::{DateTime, Utc};
use rd_core::{
    ByteCount, ListingLimit, MAX_REMOTE_DEPTH, MAX_REMOTE_ENTRIES, RemoteEntry, RemoteListing,
    is_safe_relative_path,
};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;

/// Walks `root` recursively and returns everything below it.
///
/// Bounded on entry count and depth for the same reason as the FTP walker: a symlink loop
/// would otherwise never terminate, and a large mirror produces a listing no review UI can
/// render. Hitting either limit is recorded rather than silently truncating.
pub async fn walk(sftp: &SftpSession, root: &str) -> anyhow::Result<RemoteListing> {
    let mut entries: Vec<RemoteEntry> = Vec::new();
    let mut truncated = None;
    let mut queue: Vec<(String, usize)> = vec![(String::new(), 0)];

    while let Some((relative, depth)) = queue.pop() {
        if entries.len() >= MAX_REMOTE_ENTRIES {
            truncated = Some(ListingLimit::EntryCount);
            break;
        }
        let absolute = join(root, &relative);
        let listed = match sftp.read_dir(absolute).await {
            Ok(listed) => listed,
            // A directory that became unreadable mid-walk should not lose the entries
            // already collected from its siblings.
            Err(error) => {
                tracing::warn!(depth, %error, "skipping unreadable SFTP directory");
                continue;
            }
        };
        for entry in listed {
            let name = entry.file_name();
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if !is_safe_relative_path(&path) {
                tracing::warn!(depth, "skipping unsafe remote path in SFTP listing");
                continue;
            }
            if entries.len() >= MAX_REMOTE_ENTRIES {
                truncated = Some(ListingLimit::EntryCount);
                break;
            }
            let metadata = entry.metadata();
            let is_dir = metadata.is_dir();
            // A symlink is not followed: the server resolves it on access, so walking it
            // here would either double-count a tree or leave the root entirely.
            let is_symlink = metadata.is_symlink();
            entries.push(RemoteEntry {
                path: path.clone(),
                is_dir,
                size: (!is_dir).then(|| size_of(&metadata)).flatten(),
                modified: modified_at(&metadata),
                etag: None,
            });
            if is_dir && !is_symlink {
                if depth + 1 > MAX_REMOTE_DEPTH {
                    truncated = Some(ListingLimit::Depth);
                } else {
                    queue.push((path, depth + 1));
                }
            }
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(RemoteListing {
        root: root.to_owned(),
        single_file: false,
        entries,
        // SFTP reads from an explicit offset, so resume is always available.
        supports_resume: true,
        truncated,
    })
}

pub fn size_of(metadata: &FileAttributes) -> Option<ByteCount> {
    metadata.size.and_then(|size| ByteCount::new(size).ok())
}

pub fn modified_at(metadata: &FileAttributes) -> Option<DateTime<Utc>> {
    metadata.modified().ok().map(DateTime::<Utc>::from)
}

/// Joins a listing-relative path onto the absolute root.
pub fn join(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        return root.to_owned();
    }
    format!("{}/{relative}", root.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::join;

    #[test]
    fn paths_join_without_doubling_separators() {
        assert_eq!(join("/srv", ""), "/srv");
        assert_eq!(join("/srv", "a/b.bin"), "/srv/a/b.bin");
        assert_eq!(join("/srv/", "a.bin"), "/srv/a.bin");
        assert_eq!(join("/", "a.bin"), "/a.bin");
    }
}

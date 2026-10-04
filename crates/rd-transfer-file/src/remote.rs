//! What FTP, SFTP and object storage share before a byte moves (audit 1.9.1, TR-11, TR-14): the
//! live parallelism and timeout of the remote settings, the stored login for a target, and the
//! bounded walk that turns a remote directory into a reviewable listing.
//!
//! The FTP and SFTP walkers were 86 lines in 100 the same, and both carried the same mistake —
//! "breadth first" over a `Vec` taken from the back, which is depth first, so the entry cap
//! was spent on one deep subtree before a shallow sibling was listed at all.

use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use rd_core::{
    ListingLimit, MAX_REMOTE_DEPTH, MAX_REMOTE_ENTRIES, RemoteCredential, RemoteCredentialId,
    RemoteEntry, RemoteSettings, RemoteTarget, is_safe_relative_path,
};
use rd_db::Database;
use tokio::sync::RwLock;

/// The remote-transfer settings as the API handlers and the runners share them.
pub type SharedRemoteSettings = Arc<RwLock<RemoteSettings>>;

/// The parallelism and the timeout of the remote settings, read live, so a settings change
/// needs no restart.
///
/// The callers are synchronous and the settings sit behind an async lock, so the read is a
/// `try_read`; while a writer holds the lock the value read last stands in, as in
/// `rd_tools::LiveSlots` — a fixed 2 and 60 s used to, whatever was configured (re-audit
/// 1.9.1, RA-TR-06). Clones share what was read last.
#[derive(Clone)]
pub struct LiveRemoteSettings {
    settings: SharedRemoteSettings,
    last_parallel: Arc<AtomicUsize>,
    last_timeout_ms: Arc<AtomicU64>,
}

impl LiveRemoteSettings {
    /// Reads `settings` once, so the first value standing in is the configured one.
    #[must_use]
    pub fn new(settings: SharedRemoteSettings) -> Self {
        let defaults = RemoteSettings::default().sanitized();
        let live = Self {
            settings,
            last_parallel: Arc::new(AtomicUsize::new(defaults.remote_max_parallel as usize)),
            last_timeout_ms: Arc::new(AtomicU64::new(millis(defaults.timeout()))),
        };
        let _ = (live.max_parallel(), live.timeout());
        live
    }

    /// The settings themselves, for what is read in an async context.
    #[must_use]
    pub const fn settings(&self) -> &SharedRemoteSettings {
        &self.settings
    }

    /// Concurrent transfers of one protocol.
    #[must_use]
    pub fn max_parallel(&self) -> usize {
        match self.settings.try_read() {
            Ok(settings) => {
                let parallel = settings.sanitized().remote_max_parallel as usize;
                self.last_parallel.store(parallel, Ordering::Release);
                parallel
            }
            Err(_) => self.last_parallel.load(Ordering::Acquire),
        }
    }

    /// The connect and read timeout.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        match self.settings.try_read() {
            Ok(settings) => {
                let timeout = settings.sanitized().timeout();
                self.last_timeout_ms
                    .store(millis(timeout), Ordering::Release);
                timeout
            }
            Err(_) => Duration::from_millis(self.last_timeout_ms.load(Ordering::Acquire)),
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The stored login for a target: the one pinned on the job, or the best match.
pub async fn credential_for(
    database: &Database,
    pinned: Option<RemoteCredentialId>,
    target: &RemoteTarget,
) -> Result<Option<RemoteCredential>> {
    match pinned {
        Some(id) => database.remote_credential(id).await,
        None => database.match_remote_credential(target).await,
    }
}

/// One entry of a remote directory as its protocol reported it.
pub struct ListedEntry {
    /// The name inside its directory; `.`, `..` and empty names are dropped by the walk.
    pub name: String,
    /// Everything but `path`, which the walk fills in.
    pub entry: RemoteEntry,
    /// Whether the walk may descend into it: a directory that is not a symlink. A symlink is
    /// resolved by the server on access, so following it here would double-count a tree or
    /// leave the root entirely.
    pub descend: bool,
}

/// How one protocol lists a directory.
pub trait DirectoryLister {
    /// The entries of the directory at `absolute`; `Ok(None)` skips a directory that could
    /// not be read, an error ends the walk.
    fn list(&mut self, absolute: &str) -> impl Future<Output = Result<Option<Vec<ListedEntry>>>>;
}

/// Everything below `root`, breadth first, with the reason the walk stopped early if it did.
///
/// Bounded on two axes, and hitting either is recorded rather than returning a silently short
/// list: a directory loop through symlinks would otherwise never terminate, and a large mirror
/// produces a listing no review can render. Breadth first, so a shallow wide tree is complete
/// before depth is spent. `protocol` only names the walk in the log.
pub async fn walk(
    lister: &mut impl DirectoryLister,
    root: &str,
    protocol: &str,
) -> Result<(Vec<RemoteEntry>, Option<ListingLimit>)> {
    let mut entries: Vec<RemoteEntry> = Vec::new();
    let mut truncated = None;
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(String::new(), 0)]);

    while let Some((relative, depth)) = queue.pop_front() {
        if entries.len() >= MAX_REMOTE_ENTRIES {
            truncated = Some(ListingLimit::EntryCount);
            break;
        }
        let Some(listed) = lister.list(&join(root, &relative)).await? else {
            continue;
        };
        for listed in listed {
            let name = listed.name;
            // `.` and `..` are listed by some servers and would walk the tree upwards.
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            // A server may name anything at all; a path that would escape the destination
            // never reaches the review, let alone the disk.
            if !is_safe_relative_path(&path) {
                tracing::warn!(depth, protocol, "skipping unsafe remote path in a listing");
                continue;
            }
            if entries.len() >= MAX_REMOTE_ENTRIES {
                truncated = Some(ListingLimit::EntryCount);
                break;
            }
            entries.push(RemoteEntry {
                path: path.clone(),
                ..listed.entry
            });
            if listed.descend {
                if depth + 1 > MAX_REMOTE_DEPTH {
                    truncated = Some(ListingLimit::Depth);
                } else {
                    queue.push_back((path, depth + 1));
                }
            }
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((entries, truncated))
}

/// Joins a listing-relative path onto the absolute root.
#[must_use]
pub fn join(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        return root.to_owned();
    }
    format!("{}/{relative}", root.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rd_core::{ListingLimit, MAX_REMOTE_ENTRIES, RemoteEntry};

    use super::{DirectoryLister, ListedEntry, join, walk};

    /// A remote tree held in memory: directory path to its entry names, a trailing `/` marking
    /// a directory.
    struct Tree(HashMap<String, Vec<String>>);

    impl DirectoryLister for Tree {
        async fn list(&mut self, absolute: &str) -> anyhow::Result<Option<Vec<ListedEntry>>> {
            Ok(self.0.get(absolute).map(|names| {
                names
                    .iter()
                    .map(|name| {
                        let is_dir = name.ends_with('/');
                        ListedEntry {
                            name: name.trim_end_matches('/').to_owned(),
                            entry: RemoteEntry {
                                path: String::new(),
                                is_dir,
                                size: None,
                                modified: None,
                                etag: None,
                            },
                            descend: is_dir,
                        }
                    })
                    .collect()
            }))
        }
    }

    #[test]
    fn paths_join_without_doubling_separators() {
        assert_eq!(join("/srv", ""), "/srv");
        assert_eq!(join("/srv", "a/b.bin"), "/srv/a/b.bin");
        assert_eq!(join("/srv/", "a.bin"), "/srv/a.bin");
        assert_eq!(join("/", "a.bin"), "/a.bin");
    }

    /// TR-11: the entry cap cuts the deepest level. Taken from the back, the walk went down
    /// the last directory first, spent the cap there, and never listed its shallow sibling.
    #[tokio::test]
    async fn the_walk_is_breadth_first_so_the_cap_cuts_depth() {
        let mut tree = HashMap::new();
        tree.insert(
            "/r".to_owned(),
            vec!["wide/".to_owned(), "deep/".to_owned()],
        );
        tree.insert("/r/wide".to_owned(), vec!["file.bin".to_owned()]);
        tree.insert("/r/deep".to_owned(), vec!["deeper/".to_owned()]);
        tree.insert(
            "/r/deep/deeper".to_owned(),
            (0..MAX_REMOTE_ENTRIES)
                .map(|index| format!("f{index}"))
                .collect(),
        );
        let (entries, truncated) = walk(&mut Tree(tree), "/r", "test").await.expect("walk");
        assert_eq!(truncated, Some(ListingLimit::EntryCount));
        assert_eq!(entries.len(), MAX_REMOTE_ENTRIES);
        assert!(
            entries.iter().any(|entry| entry.path == "wide/file.bin"),
            "the shallow sibling is listed before depth is spent"
        );
    }

    #[tokio::test]
    async fn unsafe_names_and_dot_entries_are_skipped_and_unreadable_directories_too() {
        let mut tree = HashMap::new();
        tree.insert(
            "/r".to_owned(),
            vec![
                ".".to_owned(),
                "..".to_owned(),
                "../escape".to_owned(),
                "gone/".to_owned(),
                "ok.bin".to_owned(),
            ],
        );
        let (entries, truncated) = walk(&mut Tree(tree), "/r", "test").await.expect("walk");
        let paths = entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["gone", "ok.bin"]);
        assert_eq!(truncated, None);
    }

    /// RA-TR-06: while a writer holds the settings, the value read last stands in, not a
    /// fixed 2 and 60 s.
    #[tokio::test]
    async fn a_held_settings_lock_answers_what_was_read_last() {
        let settings: super::SharedRemoteSettings =
            std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings {
                remote_max_parallel: 5,
                remote_timeout_seconds: 300,
                remote_ssh_auto_trust: false,
            }));
        let live = super::LiveRemoteSettings::new(settings.clone());
        let clone = live.clone();

        let mut writer = settings.write().await;
        assert_eq!(live.max_parallel(), 5);
        assert_eq!(live.timeout(), std::time::Duration::from_secs(300));

        // A change is read once the writer is gone, and clones share it.
        writer.remote_max_parallel = 7;
        writer.remote_timeout_seconds = 30;
        drop(writer);
        assert_eq!(live.max_parallel(), 7);
        assert_eq!(live.timeout(), std::time::Duration::from_secs(30));
        let _held = settings.write().await;
        assert_eq!(clone.max_parallel(), 7);
        assert_eq!(clone.timeout(), std::time::Duration::from_secs(30));
    }
}

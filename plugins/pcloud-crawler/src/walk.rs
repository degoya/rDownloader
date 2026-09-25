//! Walking a folder tree under limits it cannot talk its way out of.
//!
//! The limits are the ones `plugins/premiumize-crawler/src/walk.rs` established and every
//! cloud drive since has kept, deliberately unchanged: what a person gets back from pasting a
//! folder should not depend on which cloud it was in. Depth, count, folders, and a set of
//! visited ids against a folder that contains itself.
//!
//! **There is deliberately no page limit here, and that is a measurement rather than an
//! omission.** RD-106-04's fourth rule added one because Google Drive, Microsoft Graph and
//! Dropbox all answer a listing a page at a time and hand back a cursor. pCloud does not:
//! `listfolder` takes no `offset`, `limit` or cursor of any kind and answers a folder whole,
//! and `showpublink` answers a public folder link with its **entire tree** in one document.
//! So there is no cursor to carry and no page to cap; what bounds a wide answer instead is the
//! manifest's `max_response_bytes`, and what bounds a deep one is `MAX_DEPTH` below. A page
//! limit written here would be a limit on something that never happens, which is worse than no
//! limit at all because it would read as protection.
//!
//! The walk is keyed by `folderid` rather than by name, which is what makes it work for both
//! sources: a folder fetched from `listfolder` and a folder already sitting in the tree
//! `showpublink` answered with are the same node to it.

use crate::listing::Entry;

/// How many levels below the crawled address are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most folders one crawl reads, including the one it was given.
pub const MAX_FOLDERS: usize = 100;

/// A folder still to be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub folder_id: u64,
    /// Path relative to the crawled address, for the package suggestion; empty for the address
    /// itself until the caller names it.
    pub path: String,
    pub depth: u32,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub folder_id: u64,
    pub file_id: u64,
    pub name: String,
    /// The folder path this file sat in, relative to the crawled address.
    pub path: String,
    pub size: Option<u64>,
}

/// Which limit stopped the walk short, when one did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    Depth,
    Files,
    Folders,
}

/// The state of one crawl: the folders still to read, and what has been found so far.
pub struct Walk {
    queue: Vec<Pending>,
    /// Every folder ever enqueued, by `folderid`. A folder reached twice is read once; this is
    /// the cycle guard, and an identifier is a stronger one than a name.
    seen: Vec<u64>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the folder the crawled address named.
    #[must_use]
    pub fn start(root: u64) -> Self {
        Self {
            queue: vec![Pending {
                folder_id: root,
                path: String::new(),
                depth: 0,
            }],
            seen: vec![root],
            files: Vec::new(),
            read: 0,
            limit: None,
        }
    }

    /// The next folder to read, or `None` when the walk is over.
    pub fn next_folder(&mut self) -> Option<Pending> {
        if self.is_full() {
            self.limit.get_or_insert(Limit::Files);
            return None;
        }
        if self.read >= MAX_FOLDERS {
            self.limit.get_or_insert(Limit::Folders);
            return None;
        }
        if self.queue.is_empty() {
            return None;
        }
        self.read += 1;
        // Breadth first: the files nearest the address a person pasted are the ones they meant,
        // so if a limit does cut the walk short it cuts the far end off.
        Some(self.queue.remove(0))
    }

    /// Whether the file limit has been reached.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.files.len() >= MAX_FILES
    }

    /// Records a limit the caller hit.
    pub fn note(&mut self, limit: Limit) {
        self.limit.get_or_insert(limit);
    }

    /// Takes what one folder held.
    pub fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>) {
        for entry in entries {
            match entry {
                Entry::Folder { folder_id, name } => {
                    if folder.depth + 1 > MAX_DEPTH {
                        self.limit.get_or_insert(Limit::Depth);
                        continue;
                    }
                    if self.seen.contains(&folder_id) {
                        continue;
                    }
                    self.seen.push(folder_id);
                    self.queue.push(Pending {
                        folder_id,
                        path: join(&folder.path, &name),
                        depth: folder.depth + 1,
                    });
                }
                Entry::File {
                    file_id,
                    name,
                    size,
                } => {
                    if self.is_full() {
                        self.limit.get_or_insert(Limit::Files);
                        continue;
                    }
                    self.files.push(Found {
                        folder_id: folder.folder_id,
                        file_id,
                        name,
                        path: folder.path.clone(),
                        size,
                    });
                }
            }
        }
    }

    /// The limit that cut this walk short, if one did.
    #[must_use]
    pub const fn limit(&self) -> Option<Limit> {
        self.limit
    }

    /// How many folders were read.
    #[must_use]
    pub const fn folders_read(&self) -> usize {
        self.read
    }

    /// The folders still to read.
    #[must_use]
    pub fn pending(&self) -> &[Pending] {
        &self.queue
    }

    /// What the walk found.
    #[must_use]
    pub fn into_files(self) -> Vec<Found> {
        self.files
    }
}

/// Joins a folder path with a child name, keeping the result a relative path.
pub fn join(path: &str, name: &str) -> String {
    // A name a stranger chose must not be able to leave the path it belongs to, so the two
    // characters that would let it are dropped rather than escaped.
    let safe: String = name
        .chars()
        .filter(|character| !matches!(character, '/' | '\\') && !character.is_control())
        .collect();
    let safe = safe.trim().trim_matches('.').trim();
    if safe.is_empty() {
        return path.to_owned();
    }
    if path.is_empty() {
        return safe.chars().take(120).collect();
    }
    format!("{path}/{}", safe.chars().take(120).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::{Limit, MAX_DEPTH, MAX_FILES, MAX_FOLDERS, Walk};
    use crate::listing::Entry;

    fn folder(folder_id: u64) -> Entry {
        Entry::Folder {
            folder_id,
            name: format!("d{folder_id}"),
        }
    }

    fn file(file_id: u64) -> Entry {
        Entry::File {
            file_id,
            name: format!("f{file_id}.bin"),
            size: Some(1),
        }
    }

    /// A tree deeper than the limit is cut at the limit, not walked to the bottom.
    #[test]
    fn the_depth_limit_stops_the_descent() {
        let mut walk = Walk::start(0);
        let mut depth = 0;
        let mut next_id = 1;
        while let Some(pending) = walk.next_folder() {
            depth = pending.depth;
            walk.absorb(&pending, vec![folder(next_id), file(next_id + 1000)]);
            next_id += 1;
        }
        assert_eq!(depth, MAX_DEPTH);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        assert_eq!(walk.into_files().len() as u32, MAX_DEPTH + 1);
    }

    /// A folder holding more files than the limit hands back the limit, and says so.
    #[test]
    fn the_count_limit_stops_the_listing() {
        let mut walk = Walk::start(0);
        let pending = walk.next_folder().expect("the root");
        let entries: Vec<Entry> = (0..MAX_FILES as u64 + 50).map(file).collect();
        walk.absorb(&pending, entries);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert!(walk.is_full());
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// A folder that contains itself is read once — by identifier, which is a stronger guard
    /// than a name and the one pCloud makes available.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = Walk::start(5);
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            assert!(reads <= MAX_FOLDERS, "the walk did not terminate");
            walk.absorb(&pending, vec![folder(5), file(1)]);
        }
        assert_eq!(reads, 1, "a folder that contains itself is read once");
        let mut walk = Walk::start(1);
        let root = walk.next_folder().expect("the root");
        walk.absorb(&root, vec![folder(2), folder(2)]);
        assert_eq!(walk.pending().len(), 1, "the same id twice is one folder");
    }

    /// A folder wider than the folder limit stops at it rather than reading for ever.
    #[test]
    fn the_folder_limit_stops_the_walk() {
        let mut walk = Walk::start(0);
        let root = walk.next_folder().expect("the root");
        let children: Vec<Entry> = (1..MAX_FOLDERS as u64 + 50).map(folder).collect();
        walk.absorb(&root, children);
        while let Some(pending) = walk.next_folder() {
            walk.absorb(&pending, vec![file(pending.folder_id + 10_000)]);
        }
        assert_eq!(walk.folders_read(), MAX_FOLDERS);
        assert_eq!(walk.limit(), Some(Limit::Folders));
    }

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_folder_name_stays_inside_the_path() {
        let mut walk = Walk::start(0);
        let pending = walk.next_folder().expect("the root");
        walk.absorb(
            &pending,
            vec![
                Entry::Folder {
                    folder_id: 1,
                    name: "..etc".to_owned(),
                },
                Entry::Folder {
                    folder_id: 2,
                    name: "  ".to_owned(),
                },
                Entry::Folder {
                    folder_id: 3,
                    name: "a\u{0}b".to_owned(),
                },
                Entry::Folder {
                    folder_id: 4,
                    name: "a/../b".to_owned(),
                },
            ],
        );
        let paths: Vec<String> = std::iter::from_fn(|| walk.next_folder())
            .map(|pending| pending.path)
            .collect();
        assert_eq!(paths, vec!["etc", "", "ab", "a..b"]);
    }
}

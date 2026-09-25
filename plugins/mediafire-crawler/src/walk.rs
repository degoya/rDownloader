//! Walking a folder tree under limits it cannot talk its way out of.
//!
//! The four limits are the ones `plugins/premiumize-crawler/src/walk.rs` established and the
//! Google Drive, OneDrive and Dropbox crawlers kept, deliberately unchanged: what a person
//! gets back from pasting a folder should not depend on which cloud it was in. Depth, count,
//! folders, and a set of visited keys against a folder that contains itself. Plus the one a
//! chunked API needs: **chunks** — `folder/get_content` answers a hundred entries at a time
//! and says `more_chunks`, so a folder wide enough produces chunks for as long as anybody
//! asks. The guest reads a folder's chunks and hands the walk the whole folder at once,
//! saying whether it stopped short; the walk records that as its own limit.
//!
//! Breadth first: the files nearest the address a person pasted are the ones they meant, so
//! if a limit does cut the walk short it cuts the far end off. The fuel and time budget in
//! `manifest.toml` sit underneath as the last resort; they stop a runaway, they do not bound
//! a correct crawl.

use crate::listing::Entry;

/// How many levels below the crawled address are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most folders one crawl reads, including the one it was given.
pub const MAX_FOLDERS: usize = 100;
/// Most chunks of one folder's listing (per content type) that are followed.
pub const MAX_CHUNKS: u32 = 10;

/// A folder still to be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub key: String,
    /// Path relative to the crawled address, the crawled folder's own name first, for the
    /// package suggestion.
    pub path: String,
    pub depth: u32,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub key: String,
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
    Chunks,
}

/// The state of one crawl: the folders still to read, and what was found so far.
pub struct Walk {
    queue: Vec<Pending>,
    /// Every folder ever enqueued, by key. A folder reached twice is read once; this is the
    /// cycle guard.
    seen: Vec<String>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the folder the crawled address named, under its own name.
    #[must_use]
    pub fn start(key: &str, name: &str) -> Self {
        Self {
            queue: vec![Pending {
                key: key.to_owned(),
                path: join("", name),
                depth: 0,
            }],
            seen: vec![key.to_owned()],
            files: Vec::new(),
            read: 0,
            limit: None,
        }
    }

    /// The next folder to fetch, or `None` when the walk is over.
    pub fn next_folder(&mut self) -> Option<Pending> {
        if self.files.len() >= MAX_FILES {
            self.limit.get_or_insert(Limit::Files);
            return None;
        }
        if self.queue.is_empty() {
            return None;
        }
        if self.read >= MAX_FOLDERS {
            self.limit.get_or_insert(Limit::Folders);
            return None;
        }
        self.read += 1;
        Some(self.queue.remove(0))
    }

    /// Takes in one folder's content: its files are kept up to the limit, its subfolders are
    /// queued up to the depth limit, and `truncated` says the guest stopped reading chunks.
    pub fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>, truncated: bool) {
        if truncated {
            self.limit.get_or_insert(Limit::Chunks);
        }
        for entry in entries {
            match entry {
                Entry::File { key, name, size } => {
                    if self.files.len() >= MAX_FILES {
                        self.limit.get_or_insert(Limit::Files);
                        break;
                    }
                    self.files.push(Found {
                        key,
                        name,
                        path: folder.path.clone(),
                        size,
                    });
                }
                Entry::Folder { key, name } => {
                    if folder.depth >= MAX_DEPTH {
                        self.limit.get_or_insert(Limit::Depth);
                        continue;
                    }
                    if self.seen.contains(&key) {
                        continue;
                    }
                    self.seen.push(key.clone());
                    self.queue.push(Pending {
                        key,
                        path: join(&folder.path, &name),
                        depth: folder.depth + 1,
                    });
                }
            }
        }
    }

    /// Which limit cut the walk short, if one did.
    #[must_use]
    pub fn limit(&self) -> Option<Limit> {
        self.limit
    }

    /// Everything found, in the order it was found.
    #[must_use]
    pub fn into_files(self) -> Vec<Found> {
        self.files
    }
}

/// `path/name`, with the separators a stranger's name may carry replaced, so a name can
/// never leave the folder it belongs to.
#[must_use]
pub fn join(path: &str, name: &str) -> String {
    let name: String = name
        .chars()
        .map(|character| {
            if matches!(character, '/' | '\\') {
                '_'
            } else {
                character
            }
        })
        .collect();
    let name = name.trim();
    if path.is_empty() {
        name.to_owned()
    } else if name.is_empty() {
        path.to_owned()
    } else {
        format!("{path}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::{Limit, MAX_DEPTH, MAX_FILES, MAX_FOLDERS, Walk, join};
    use crate::listing::Entry;

    fn file(name: &str) -> Entry {
        Entry::File {
            key: format!("k{name}"),
            name: name.to_owned(),
            size: Some(1),
        }
    }

    fn folder(key: &str, name: &str) -> Entry {
        Entry::Folder {
            key: key.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn a_tree_is_walked_breadth_first_with_paths_from_the_root_name() {
        let mut walk = Walk::start("root", "Walls");
        let root = walk.next_folder().expect("root");
        assert_eq!((root.path.as_str(), root.depth), ("Walls", 0));
        walk.absorb(
            &root,
            vec![file("a.png"), folder("sub", "Test/Folder")],
            false,
        );
        let sub = walk.next_folder().expect("subfolder");
        assert_eq!((sub.path.as_str(), sub.depth), ("Walls/Test_Folder", 1));
        walk.absorb(&sub, vec![file("b.png")], false);
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.limit(), None);
        let files = walk.into_files();
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].path, "Walls/Test_Folder");
    }

    #[test]
    fn a_folder_that_contains_itself_is_read_once() {
        let mut walk = Walk::start("root", "Loop");
        let root = walk.next_folder().expect("root");
        walk.absorb(&root, vec![folder("root", "Loop"), folder("x", "X")], false);
        let x = walk.next_folder().expect("x");
        walk.absorb(&x, vec![folder("root", "Loop again")], false);
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.limit(), None);
    }

    #[test]
    fn every_limit_is_named() {
        let mut walk = Walk::start("root", "Deep");
        let mut pending = walk.next_folder().expect("root");
        for level in 0..=MAX_DEPTH {
            walk.absorb(&pending, vec![folder(&format!("d{level}"), "d")], false);
            match walk.next_folder() {
                Some(next) => pending = next,
                None => break,
            }
        }
        assert_eq!(walk.limit(), Some(Limit::Depth));

        let mut walk = Walk::start("root", "Wide");
        let root = walk.next_folder().expect("root");
        let files = (0..=MAX_FILES)
            .map(|index| file(&index.to_string()))
            .collect();
        walk.absorb(&root, files, false);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert_eq!(walk.into_files().len(), MAX_FILES);

        let mut walk = Walk::start("root", "Many");
        let root = walk.next_folder().expect("root");
        let folders = (0..MAX_FOLDERS)
            .map(|index| folder(&index.to_string(), "f"))
            .collect();
        walk.absorb(&root, folders, false);
        while let Some(pending) = walk.next_folder() {
            walk.absorb(&pending, Vec::new(), false);
        }
        assert_eq!(walk.limit(), Some(Limit::Folders));

        let mut walk = Walk::start("root", "Chunked");
        let root = walk.next_folder().expect("root");
        walk.absorb(&root, vec![file("a")], true);
        assert_eq!(walk.limit(), Some(Limit::Chunks));
    }

    #[test]
    fn join_keeps_a_name_inside_its_folder() {
        assert_eq!(join("", "Walls"), "Walls");
        assert_eq!(join("Walls", "Season 1"), "Walls/Season 1");
        assert_eq!(join("Walls", "../../etc"), "Walls/.._.._etc");
        assert_eq!(join("Walls", "  "), "Walls");
    }
}

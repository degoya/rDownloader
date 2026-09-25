//! Walking a folder tree under limits it cannot talk its way out of.
//!
//! The four limits are not politeness. A crawler follows addresses a stranger controls, so
//! every failure mode below is somebody else's to trigger:
//!
//! - **Depth** — a tree nested a thousand deep costs a request per level.
//! - **Count** — a folder holding a million entries fills the review list with them.
//! - **Pages** — Graph answers `/children` a page at a time and hands back an
//!   `@odata.nextLink`. A folder wide enough produces pages for as long as anybody asks, so
//!   the link is followed a bounded number of times and no further.
//! - **Cycles** — a shortcut can point at an ancestor, so a folder can contain itself through
//!   three others. This is the one that cannot be caught by "it is taking a while": every
//!   single request looks perfectly reasonable.
//!
//! So the walk carries its own bookkeeping and refuses rather than trusts. The fuel and time
//! budget in `manifest.toml` sit underneath as the last resort; they stop a runaway, they do
//! not bound a correct crawl, and a plugin that relied on them would report a timeout where it
//! should report "this folder is bigger than I will list".
//!
//! The numbers are the ones `plugins/premiumize-crawler/src/walk.rs` established and
//! `plugins/google-drive-crawler/src/walk.rs` extended by the page cap, deliberately
//! unchanged: what a person gets back from pasting a folder should not depend on which cloud
//! it was in.

use crate::listing::Entry;

/// How many levels below the crawled address are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most folders one crawl reads, including the one it was given.
pub const MAX_FOLDERS: usize = 100;
/// Most pages of one folder's listing that are followed.
pub const MAX_PAGES: usize = 10;

/// A folder still to be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    /// The item id, or empty for the shared root itself, which is reached by the share alone.
    pub id: String,
    /// Path relative to the crawled address; empty for the address itself.
    pub path: String,
    pub depth: u32,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub id: String,
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
    Pages,
}

/// The state of one crawl.
pub struct Walk {
    queue: Vec<Pending>,
    /// Every folder id ever enqueued. A folder reached twice is read once — this is the cycle
    /// guard, and it is by id rather than by path because a cycle changes the path every time
    /// round and would otherwise look like new ground for ever.
    seen: Vec<String>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the shared root, whose own item id is `root_id` once it is known.
    #[must_use]
    pub fn start(root_id: &str) -> Self {
        Self {
            queue: vec![Pending {
                id: String::new(),
                path: String::new(),
                depth: 0,
            }],
            seen: vec![root_id.to_owned()],
            files: Vec::new(),
            read: 0,
            limit: None,
        }
    }

    /// The next folder to fetch, or `None` when the walk is over.
    pub fn next_folder(&mut self) -> Option<Pending> {
        if self.is_full() {
            self.limit.get_or_insert(Limit::Files);
            return None;
        }
        if self.read >= MAX_FOLDERS {
            self.limit.get_or_insert(Limit::Folders);
            return None;
        }
        // Breadth first: the files nearest the address a person pasted are the ones they meant,
        // so if a limit does cut the walk short it cuts the far end off.
        if self.queue.is_empty() {
            return None;
        }
        self.read += 1;
        Some(self.queue.remove(0))
    }

    /// Whether the file limit has been reached, which is what stops one folder's pagination as
    /// well as the walk itself.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.files.len() >= MAX_FILES
    }

    /// Records a limit the caller hit — the page cap, which only the caller can see.
    pub fn note(&mut self, limit: Limit) {
        self.limit.get_or_insert(limit);
    }

    /// Takes what one page of one folder held.
    pub fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>) {
        for entry in entries {
            match entry {
                Entry::Folder { id, name } => {
                    if folder.depth + 1 > MAX_DEPTH {
                        self.limit.get_or_insert(Limit::Depth);
                        continue;
                    }
                    if self.seen.iter().any(|known| known == &id) {
                        continue;
                    }
                    self.seen.push(id.clone());
                    self.queue.push(Pending {
                        path: join(&folder.path, &name),
                        id,
                        depth: folder.depth + 1,
                    });
                }
                Entry::File { id, name, size } => {
                    if self.is_full() {
                        self.limit.get_or_insert(Limit::Files);
                        continue;
                    }
                    self.files.push(Found {
                        id,
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

    fn folder(id: &str) -> Entry {
        Entry::Folder {
            id: id.to_owned(),
            name: format!("dir-{id}"),
        }
    }

    fn file(name: &str) -> Entry {
        Entry::File {
            id: name.to_owned(),
            name: name.to_owned(),
            size: Some(1),
        }
    }

    /// A tree deeper than the limit is cut at the limit, not walked to the bottom.
    #[test]
    fn the_depth_limit_stops_the_descent() {
        let mut walk = Walk::start("root");
        let mut depth = 0;
        while let Some(pending) = walk.next_folder() {
            depth = pending.depth;
            walk.absorb(
                &pending,
                vec![folder(&format!("d{}", pending.depth + 1)), file("a")],
            );
        }
        assert_eq!(depth, MAX_DEPTH);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        assert_eq!(walk.folders_read() as u32, MAX_DEPTH + 1);
    }

    /// A folder with more files than the limit hands back the limit and says so.
    #[test]
    fn the_file_limit_stops_the_walk_and_is_reported() {
        let mut walk = Walk::start("root");
        let pending = walk.next_folder().expect("root");
        let entries: Vec<Entry> = (0..MAX_FILES + 50)
            .map(|index| file(&format!("f{index}")))
            .collect();
        walk.absorb(&pending, entries);
        assert!(walk.is_full());
        assert_eq!(walk.next_folder(), None);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// More subfolders than the limit are read up to the limit, breadth first.
    #[test]
    fn the_folder_limit_stops_the_walk_and_is_reported() {
        let mut walk = Walk::start("root");
        let pending = walk.next_folder().expect("root");
        let entries: Vec<Entry> = (0..MAX_FOLDERS + 20)
            .map(|index| folder(&format!("s{index}")))
            .collect();
        walk.absorb(&pending, entries);
        let mut read = 1;
        while let Some(pending) = walk.next_folder() {
            read += 1;
            walk.absorb(&pending, vec![file(&format!("in-{}", pending.id))]);
        }
        assert_eq!(read, MAX_FOLDERS);
        assert_eq!(walk.limit(), Some(Limit::Folders));
    }

    /// A folder that contains itself is read once.
    #[test]
    fn a_cycle_is_walked_once() {
        let mut walk = Walk::start("root");
        let root = walk.next_folder().expect("root");
        walk.absorb(&root, vec![folder("inner"), file("a")]);
        let inner = walk.next_folder().expect("inner");
        walk.absorb(&inner, vec![folder("root"), folder("inner"), file("b")]);
        assert_eq!(walk.next_folder(), None);
        assert_eq!(walk.folders_read(), 2);
        assert_eq!(walk.limit(), None);
        assert_eq!(walk.into_files().len(), 2);
    }

    /// The path a file is reported under is rooted in the crawled folder's own name, and a
    /// stranger's name cannot leave it.
    #[test]
    fn the_package_hint_is_the_path_below_the_crawled_folder() {
        let mut walk = Walk::start("root");
        let mut root = walk.next_folder().expect("root");
        root.path = super::join("", "Show");
        walk.absorb(
            &root,
            vec![
                Entry::Folder {
                    id: "s1".to_owned(),
                    name: "../Season 1/".to_owned(),
                },
                file("readme.txt"),
            ],
        );
        let season = walk.next_folder().expect("season");
        assert_eq!(season.path, "Show/Season 1");
        walk.absorb(&season, vec![file("e01.mkv")]);
        let files = walk.into_files();
        assert_eq!(files[0].path, "Show");
        assert_eq!(files[1].path, "Show/Season 1");
    }

    #[test]
    fn a_name_that_is_nothing_but_separators_adds_no_segment() {
        assert_eq!(super::join("Show", "///"), "Show");
        assert_eq!(super::join("", "..."), "");
        assert_eq!(super::join("a", "b\u{0}c"), "a/bc");
        assert_eq!(super::join("", &"x".repeat(200)).len(), 120);
    }
}

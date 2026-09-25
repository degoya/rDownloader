//! Walking a folder tree under limits it cannot talk its way out of.
//!
//! The three limits are not politeness. A crawler follows addresses a stranger controls, so
//! all three failure modes are somebody else's to trigger:
//!
//! - **Depth** — a tree nested a thousand deep costs a request per level.
//! - **Count** — a folder holding a million entries fills the review list with them.
//! - **Cycles** — a folder that contains itself, directly or through three others, is walked
//!   for ever. This is the one that cannot be caught by "it is taking a while": every single
//!   request looks perfectly reasonable.
//!
//! So the walk carries its own bookkeeping and refuses rather than trusts. The fuel and time
//! budget in `manifest.toml` sit underneath as the last resort; they stop a runaway, they do
//! not bound a correct crawl, and a plugin that relied on them would report a timeout where
//! it should report "this folder is bigger than I will list".

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
    pub id: String,
    /// Path relative to the crawled address; empty for the address itself.
    pub path: String,
    pub depth: u32,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub url: String,
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

/// The state of one crawl.
pub struct Walk {
    queue: Vec<Pending>,
    /// Every folder id ever enqueued. A folder reached twice is read once — this is the
    /// cycle guard, and it is by id rather than by path because a cycle changes the path
    /// every time round and would otherwise look like new ground for ever.
    seen: Vec<String>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the folder the crawled address named.
    #[must_use]
    pub fn start(root: &str) -> Self {
        Self {
            queue: vec![Pending {
                id: root.to_owned(),
                path: String::new(),
                depth: 0,
            }],
            seen: vec![root.to_owned()],
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
        if self.read >= MAX_FOLDERS {
            self.limit.get_or_insert(Limit::Folders);
            return None;
        }
        // Breadth first: the files nearest the address a person pasted are the ones they
        // meant, so if a limit does cut the walk short it cuts the far end off.
        if self.queue.is_empty() {
            return None;
        }
        self.read += 1;
        Some(self.queue.remove(0))
    }

    /// Takes what one folder held.
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
                Entry::File { name, url, size } => {
                    if self.files.len() >= MAX_FILES {
                        self.limit.get_or_insert(Limit::Files);
                        continue;
                    }
                    self.files.push(Found {
                        url,
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
fn join(path: &str, name: &str) -> String {
    // A name a stranger chose must not be able to leave the path it belongs to, so the two
    // characters that would let it are dropped rather than escaped.
    let safe: String = name
        .chars()
        .filter(|character| !matches!(character, '/' | '\\'))
        .collect();
    let safe = safe.trim().trim_matches('.').trim();
    if safe.is_empty() {
        return path.to_owned();
    }
    if path.is_empty() {
        return safe.to_owned();
    }
    format!("{path}/{safe}")
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
            name: name.to_owned(),
            url: format!("https://cdn.example.org/{name}"),
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
        // One file per level walked: the root plus MAX_DEPTH levels below it.
        assert_eq!(walk.into_files().len() as u32, MAX_DEPTH + 1);
    }

    /// A folder holding more files than the limit hands back the limit, and says so.
    #[test]
    fn the_count_limit_stops_the_listing() {
        let mut walk = Walk::start("root");
        let pending = walk.next_folder().expect("the root");
        let entries: Vec<Entry> = (0..MAX_FILES + 50)
            .map(|index| file(&format!("f{index}")))
            .collect();
        walk.absorb(&pending, entries);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// A folder that contains itself is read once. Without this the walk never ends, and
    /// every request it makes looks perfectly reasonable on its own.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = Walk::start("root");
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            assert!(reads <= MAX_FOLDERS, "the walk did not terminate");
            // Every folder claims to contain the root and a sibling that claims the same.
            walk.absorb(&pending, vec![folder("root"), folder("loop"), file("a")]);
        }
        assert_eq!(reads, 2, "root and `loop`, each read exactly once");
        assert_eq!(walk.limit(), None);
        assert_eq!(walk.into_files().len(), 2);
    }

    /// A folder wider than the folder limit stops at it rather than reading for ever.
    #[test]
    fn the_folder_limit_stops_the_walk() {
        let mut walk = Walk::start("root");
        let root = walk.next_folder().expect("the root");
        // One level down, so the depth limit cannot be what stops this: breadth is.
        let children: Vec<Entry> = (0..MAX_FOLDERS + 50)
            .map(|index| folder(&format!("n{index}")))
            .collect();
        walk.absorb(&root, children);
        while let Some(pending) = walk.next_folder() {
            walk.absorb(&pending, vec![file("a")]);
        }
        assert_eq!(walk.folders_read(), MAX_FOLDERS);
        assert_eq!(walk.limit(), Some(Limit::Folders));
    }

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_folder_name_stays_inside_the_path() {
        let mut walk = Walk::start("root");
        let pending = walk.next_folder().expect("the root");
        walk.absorb(
            &pending,
            vec![
                Entry::Folder {
                    id: "a".to_owned(),
                    name: "../../etc".to_owned(),
                },
                Entry::Folder {
                    id: "b".to_owned(),
                    name: "  ".to_owned(),
                },
            ],
        );
        let first = walk.next_folder().expect("first child");
        assert_eq!(first.path, "etc");
        let second = walk.next_folder().expect("second child");
        assert_eq!(second.path, "");
    }
}

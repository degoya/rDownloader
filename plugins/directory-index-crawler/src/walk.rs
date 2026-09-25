//! Walking an open directory tree under limits it cannot talk its way out of.
//!
//! The numbers are the ones `plugins/premiumize-crawler/src/walk.rs` established, and they
//! matter more here than there: a Premiumize folder belongs to the account that pastes it,
//! whereas an open listing belongs to a stranger. All three failure modes are therefore
//! somebody else's to trigger — a tree nested a thousand deep, a directory holding a million
//! names, and a symlink that points at its own parent, which is the one that cannot be
//! noticed by "it is taking a while" because every request looks perfectly reasonable.

use crate::listing::Entry;

/// How many levels below the crawled address are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most directories one crawl reads, including the one it was given.
pub const MAX_DIRECTORIES: usize = 100;

/// A directory still to be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub url: String,
    /// Path relative to the crawled address; empty for the address itself.
    pub path: String,
    pub depth: u32,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub url: String,
    pub name: String,
    /// The directory path this file sat in, relative to the crawled address.
    pub path: String,
    pub size: Option<u64>,
}

/// Which limit stopped the walk short, when one did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    Depth,
    Files,
    Directories,
}

/// The state of one crawl.
pub struct Walk {
    queue: Vec<Pending>,
    /// Every address ever enqueued. A directory reached twice is read once — this is the
    /// cycle guard, and a symlink loop is exactly what it is for.
    seen: Vec<String>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the address that was pasted. `root` is the name shown for it.
    #[must_use]
    pub fn start(url: &str, root: &str) -> Self {
        Self {
            queue: vec![Pending {
                url: url.to_owned(),
                path: root.to_owned(),
                depth: 0,
            }],
            seen: vec![url.to_owned()],
            files: Vec::new(),
            read: 0,
            limit: None,
        }
    }

    /// The next directory to fetch, or `None` when the walk is over.
    pub fn next_directory(&mut self) -> Option<Pending> {
        if self.files.len() >= MAX_FILES {
            self.limit.get_or_insert(Limit::Files);
            return None;
        }
        if self.read >= MAX_DIRECTORIES {
            self.limit.get_or_insert(Limit::Directories);
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

    /// Takes what one directory held.
    pub fn absorb(&mut self, directory: &Pending, found: Vec<Entry>) {
        for entry in found {
            match entry {
                Entry::Directory { url, name } => {
                    if directory.depth + 1 > MAX_DEPTH {
                        self.limit.get_or_insert(Limit::Depth);
                        continue;
                    }
                    if self.seen.iter().any(|known| known == &url) {
                        continue;
                    }
                    self.seen.push(url.clone());
                    self.queue.push(Pending {
                        path: join(&directory.path, &name),
                        url,
                        depth: directory.depth + 1,
                    });
                }
                Entry::File { url, name, size } => {
                    if self.files.len() >= MAX_FILES {
                        self.limit.get_or_insert(Limit::Files);
                        continue;
                    }
                    self.files.push(Found {
                        url,
                        name,
                        path: directory.path.clone(),
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

    /// How many directories were read.
    #[must_use]
    pub const fn directories_read(&self) -> usize {
        self.read
    }

    /// What the walk found.
    #[must_use]
    pub fn into_files(self) -> Vec<Found> {
        self.files
    }
}

/// Joins a directory path with a child name, keeping the result a relative path.
///
/// A name a stranger chose must not be able to leave the path it belongs to, so the two
/// characters that would let it are dropped rather than escaped.
#[must_use]
pub fn join(path: &str, name: &str) -> String {
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
    use super::{Limit, MAX_DEPTH, MAX_DIRECTORIES, MAX_FILES, Walk, join};
    use crate::listing::Entry;

    fn directory(name: &str) -> Entry {
        Entry::Directory {
            url: format!("https://files.example.org/pub/{name}/"),
            name: name.to_owned(),
        }
    }

    fn file(name: &str) -> Entry {
        Entry::File {
            url: format!("https://files.example.org/pub/{name}"),
            name: name.to_owned(),
            size: Some(1),
        }
    }

    fn start() -> Walk {
        Walk::start("https://files.example.org/pub/", "pub")
    }

    /// A tree deeper than the limit is cut at the limit, not walked to the bottom.
    #[test]
    fn the_depth_limit_stops_the_descent() {
        let mut walk = start();
        let mut depth = 0;
        while let Some(pending) = walk.next_directory() {
            depth = pending.depth;
            walk.absorb(
                &pending,
                vec![
                    directory(&format!("d{}", pending.depth + 1)),
                    file(&format!("f{}", pending.depth)),
                ],
            );
        }
        assert_eq!(depth, MAX_DEPTH);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        assert_eq!(walk.into_files().len() as u32, MAX_DEPTH + 1);
    }

    /// A directory holding more files than the limit hands back the limit, and says so.
    #[test]
    fn the_count_limit_stops_the_listing() {
        let mut walk = start();
        let pending = walk.next_directory().expect("the root");
        let entries: Vec<Entry> = (0..MAX_FILES + 50)
            .map(|index| file(&format!("f{index}")))
            .collect();
        walk.absorb(&pending, entries);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert!(walk.next_directory().is_none());
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// A symlink that points back at a directory already read is read once. Without this
    /// the walk never ends, and every request it makes looks perfectly reasonable.
    #[test]
    fn a_loop_of_symlinks_is_walked_once_and_not_for_ever() {
        let mut walk = start();
        let mut reads = 0;
        while let Some(pending) = walk.next_directory() {
            reads += 1;
            assert!(reads <= MAX_DIRECTORIES, "the walk did not terminate");
            walk.absorb(
                &pending,
                vec![
                    Entry::Directory {
                        url: "https://files.example.org/pub/".to_owned(),
                        name: "self".to_owned(),
                    },
                    directory("loop"),
                    file("a"),
                ],
            );
        }
        assert_eq!(reads, 2, "the root and `loop`, each read exactly once");
        assert_eq!(walk.limit(), None);
        assert_eq!(walk.into_files().len(), 2);
    }

    /// A directory wider than the limit stops at it rather than reading for ever.
    #[test]
    fn the_directory_limit_stops_the_walk() {
        let mut walk = start();
        let root = walk.next_directory().expect("the root");
        let children: Vec<Entry> = (0..MAX_DIRECTORIES + 50)
            .map(|index| directory(&format!("n{index}")))
            .collect();
        walk.absorb(&root, children);
        while let Some(pending) = walk.next_directory() {
            walk.absorb(&pending, vec![file("a")]);
        }
        assert_eq!(walk.directories_read(), MAX_DIRECTORIES);
        assert_eq!(walk.limit(), Some(Limit::Directories));
    }

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_directory_name_stays_inside_the_path() {
        assert_eq!(join("pub", "../../etc"), "pub/etc");
        assert_eq!(join("pub", "  "), "pub");
        assert_eq!(join("", "Season 1"), "Season 1");
        assert_eq!(join("pub", "a\\b"), "pub/ab");
    }
}

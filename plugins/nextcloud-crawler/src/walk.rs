//! Walking a share's folder tree under limits it cannot talk its way out of.
//!
//! The numbers are the ones `plugins/premiumize-crawler/src/walk.rs` established. They matter
//! at least as much here: a public share belongs to a stranger, so a tree nested a thousand
//! deep, a folder holding a million entries and a folder that contains itself are all
//! somebody else's to arrange. The last one cannot be noticed by "it is taking a while" —
//! every single request looks perfectly reasonable — so the walk keeps its own bookkeeping.

use crate::propfind::Item;

/// How many levels below the shared folder are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most folders one crawl reads, including the shared folder itself.
pub const MAX_FOLDERS: usize = 100;

/// A folder still to be read, addressed the way the server addressed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    /// The server-absolute, percent-encoded path from the `href` the server gave.
    pub href: String,
    /// Path relative to the crawled share; the share's own name for the share itself.
    pub path: String,
    pub depth: u32,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub href: String,
    pub name: String,
    /// The folder path this file sat in, relative to the crawled share.
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
    /// Every address ever enqueued. A folder reached twice is read once — this is the cycle
    /// guard, and it is by address because a cycle changes the display path every time round
    /// and would otherwise look like new ground for ever.
    seen: Vec<String>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the shared folder. `root` is the name shown for it.
    #[must_use]
    pub fn start(href: &str, root: &str) -> Self {
        Self {
            queue: vec![Pending {
                href: normalize(href),
                path: root.to_owned(),
                depth: 0,
            }],
            seen: vec![normalize(href)],
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

    /// Takes what one `PROPFIND` answered.
    ///
    /// The entry describing the folder itself is dropped by address rather than by position:
    /// a server answering in another order would otherwise cost a file and gain a loop.
    pub fn absorb(&mut self, folder: &Pending, items: Vec<Item>) {
        for item in items {
            let href = normalize(&item.href);
            if href == folder.href {
                continue;
            }
            if item.is_collection {
                if folder.depth + 1 > MAX_DEPTH {
                    self.limit.get_or_insert(Limit::Depth);
                    continue;
                }
                if self.seen.iter().any(|known| known == &href) {
                    continue;
                }
                self.seen.push(href.clone());
                self.queue.push(Pending {
                    path: join(&folder.path, &item.name),
                    href,
                    depth: folder.depth + 1,
                });
            } else {
                if self.files.len() >= MAX_FILES {
                    self.limit.get_or_insert(Limit::Files);
                    continue;
                }
                self.files.push(Found {
                    href,
                    name: item.name,
                    path: folder.path.clone(),
                    size: item.size,
                });
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

/// One spelling for one address, so "the folder itself" and "a folder already read" are
/// recognisable however the server wrote them.
#[must_use]
pub fn normalize(href: &str) -> String {
    let trimmed = href.trim();
    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Joins a folder path with a child name, keeping the result a relative path.
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
    use super::{Limit, MAX_DEPTH, MAX_FILES, MAX_FOLDERS, Walk, join, normalize};
    use crate::propfind::Item;

    const ROOT: &str = "/public.php/dav/files/abcdefghijklmno";

    fn folder(href: &str, name: &str) -> Item {
        Item {
            href: format!("{ROOT}/{href}/"),
            name: name.to_owned(),
            size: None,
            is_collection: true,
        }
    }

    fn file(name: &str) -> Item {
        Item {
            href: format!("{ROOT}/{name}"),
            name: name.to_owned(),
            size: Some(1),
            is_collection: false,
        }
    }

    fn start() -> Walk {
        Walk::start(&format!("{ROOT}/"), "Holiday")
    }

    /// The entry for the folder itself is not a child of it, however the server spelled it.
    #[test]
    fn the_folder_itself_is_not_one_of_its_own_entries() {
        let mut walk = start();
        let pending = walk.next_folder().expect("the share");
        assert_eq!(pending.href, ROOT, "the trailing slash is not part of it");
        walk.absorb(
            &pending,
            vec![
                Item {
                    href: format!("{ROOT}/"),
                    name: "Holiday".to_owned(),
                    size: None,
                    is_collection: true,
                },
                file("disc.iso"),
            ],
        );
        assert!(walk.next_folder().is_none(), "no folder was enqueued");
        let files = walk.into_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "Holiday", "the share name is the package");
    }

    /// A tree deeper than the limit is cut at the limit, not walked to the bottom.
    #[test]
    fn the_depth_limit_stops_the_descent() {
        let mut walk = start();
        let mut depth = 0;
        while let Some(pending) = walk.next_folder() {
            depth = pending.depth;
            let level = pending.depth + 1;
            walk.absorb(
                &pending,
                vec![
                    folder(&format!("d{level}"), &format!("d{level}")),
                    file(&format!("f{level}")),
                ],
            );
        }
        assert_eq!(depth, MAX_DEPTH);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        assert_eq!(walk.into_files().len() as u32, MAX_DEPTH + 1);
    }

    /// A folder holding more files than the limit hands back the limit, and says so.
    #[test]
    fn the_count_limit_stops_the_listing() {
        let mut walk = start();
        let pending = walk.next_folder().expect("the share");
        let items: Vec<Item> = (0..MAX_FILES + 50)
            .map(|index| file(&format!("f{index}")))
            .collect();
        walk.absorb(&pending, items);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// A folder that contains itself is read once. Without this the walk never ends, and
    /// every request it makes looks perfectly reasonable.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = start();
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            assert!(reads <= MAX_FOLDERS, "the walk did not terminate");
            walk.absorb(
                &pending,
                vec![
                    Item {
                        href: format!("{ROOT}/"),
                        name: "back to the top".to_owned(),
                        size: None,
                        is_collection: true,
                    },
                    folder("loop", "loop"),
                    file("a"),
                ],
            );
        }
        assert_eq!(reads, 2, "the share and `loop`, each read exactly once");
        assert_eq!(walk.limit(), None);
        assert_eq!(walk.into_files().len(), 2);
    }

    /// A folder wider than the folder limit stops at it rather than reading for ever.
    #[test]
    fn the_folder_limit_stops_the_walk() {
        let mut walk = start();
        let root = walk.next_folder().expect("the share");
        let children: Vec<Item> = (0..MAX_FOLDERS + 50)
            .map(|index| folder(&format!("n{index}"), &format!("n{index}")))
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
        assert_eq!(join("Holiday", "../../etc"), "Holiday/etc");
        assert_eq!(join("Holiday", "  "), "Holiday");
        assert_eq!(join("", "Season 1"), "Season 1");
        assert_eq!(normalize("/a/b/"), "/a/b");
        assert_eq!(normalize("/"), "/");
    }
}

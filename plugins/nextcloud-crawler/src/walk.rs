//! Walking a share's folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07). They matter at least as much here: a public share
//! belongs to a stranger, so a tree nested a thousand deep, a folder holding a million entries
//! and a folder that contains itself are all somebody else's to arrange. What is WebDAV's own
//! is here: a folder is named by its `href`, normalised before it reaches the walk so the cycle
//! guard sees one folder however the server spelled the trailing slash, and the entry for the
//! folder itself is dropped from its own listing. A `PROPFIND` with `Depth: 1` answers a folder
//! whole, so the shared page limit never fires here.

use crate::propfind::Item;

pub use plugin_common::walk::{Limit, join};

/// A folder still to be read, by its normalised `href`.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<String, Found>;

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    /// The file's normalised `href`.
    pub href: String,
    pub name: String,
    /// The folder path this file sat in, starting at the share's name.
    pub path: String,
    pub size: Option<u64>,
}

/// Starts at the share's own collection, with the path every file is reported under.
#[must_use]
pub fn start(href: &str, root: &str) -> Walk {
    Walk::start_at(normalize(href), root.to_owned())
}

/// Takes what one `PROPFIND` of one folder listed.
pub trait Absorb {
    fn absorb(&mut self, folder: &Pending, items: Vec<Item>);
}

impl Absorb for Walk {
    fn absorb(&mut self, folder: &Pending, items: Vec<Item>) {
        for item in items {
            let href = normalize(&item.href);
            if href == folder.id {
                continue;
            }
            if item.is_collection {
                self.enter(folder, href, &item.name);
            } else {
                self.add_file(Found {
                    href,
                    name: item.name,
                    path: folder.path.clone(),
                    size: item.size,
                });
            }
        }
    }
}

/// An `href` as the walk compares it: trimmed, without a trailing slash, `/` for the root.
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

#[cfg(test)]
mod tests {
    use super::{Absorb, Walk, join, normalize};
    use crate::propfind::Item;
    use plugin_common::walk::MAX_FOLDERS;

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
        super::start(&format!("{ROOT}/"), "Holiday")
    }

    /// The entry for the folder itself is not a child of it, however the server spelled it.
    #[test]
    fn the_folder_itself_is_not_one_of_its_own_entries() {
        let mut walk = start();
        let pending = walk.next_folder().expect("the share");
        assert_eq!(pending.id, ROOT, "the trailing slash is not part of it");
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

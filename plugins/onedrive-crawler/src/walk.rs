//! Walking a OneDrive folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07): what a person gets back from pasting a folder should not
//! depend on which cloud it was in. What is OneDrive's own is here: a folder is named by its
//! item id, and a file keeps its id, name and size. The walk starts at the shared root's own
//! id, so a folder that links back to it is not read twice; the caller reaches that root (depth
//! `0`) through the share alone. Graph answers `/children` a page at a time and hands back an
//! `@odata.nextLink`; the page cap is applied by the caller, which is the only one that sees
//! the pages.

use crate::listing::Entry;

pub use plugin_common::walk::{Limit, MAX_PAGES, join};

/// A folder still to be read, by its item id.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<String, Found>;

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub id: String,
    pub name: String,
    /// The folder path this file sat in, relative to the crawled address.
    pub path: String,
    pub size: Option<u64>,
}

/// Takes what one page of one folder held.
pub trait Absorb {
    fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>);
}

impl Absorb for Walk {
    fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>) {
        for entry in entries {
            match entry {
                Entry::Folder { id, name } => {
                    self.enter(folder, id, &name);
                }
                Entry::File { id, name, size } => {
                    self.add_file(Found {
                        id,
                        name,
                        path: folder.path.clone(),
                        size,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Absorb, Limit, Walk, join};
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

    /// A folder that contains itself is read once: the guard is the item id.
    #[test]
    fn a_cycle_is_walked_once() {
        let mut walk = Walk::start("root".to_owned());
        let root = walk.next_folder().expect("root");
        walk.absorb(&root, vec![folder("inner"), file("a")]);
        let inner = walk.next_folder().expect("inner");
        walk.absorb(&inner, vec![folder("root"), folder("inner"), file("b")]);
        assert_eq!(walk.next_folder(), None);
        assert_eq!(walk.folders_read(), 2);
        assert_eq!(walk.limit(), None::<Limit>);
        assert_eq!(walk.into_files().len(), 2);
    }

    /// The path a file is reported under is rooted in the crawled folder's own name, and a
    /// stranger's name cannot leave it.
    #[test]
    fn the_package_hint_is_the_path_below_the_crawled_folder() {
        let mut walk = Walk::start("root".to_owned());
        let mut root = walk.next_folder().expect("root");
        root.path = join("", "Show");
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
        assert_eq!(files[0].id, "readme.txt");
        assert_eq!(files[0].size, Some(1));
        assert_eq!(files[1].path, "Show/Season 1");
    }
}

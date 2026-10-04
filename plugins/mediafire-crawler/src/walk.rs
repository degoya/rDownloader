//! Walking a MediaFire folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07): what a person gets back from pasting a folder should not
//! depend on which cloud it was in. What is MediaFire's own is here: a folder and a file are
//! named by their keys, and `folder/get_content` answers a hundred entries at a time and says
//! `more_chunks`, so a folder wide enough produces chunks for as long as anybody asks. The guest
//! reads a folder's chunks up to [`MAX_CHUNKS`] — the shared page cap — and hands the walk the
//! whole folder at once, saying whether it stopped short; the walk records that as
//! [`Limit::Pages`].
//!
//! A folder's name passes through the shared `join`, which drops a separator in it rather than
//! replacing it with `_` as this crawler once did alone.

use crate::listing::Entry;

pub use plugin_common::walk::{Limit, join};

/// Most chunks of one folder's listing that are read, per content type: the shared page cap.
pub const MAX_CHUNKS: usize = plugin_common::walk::MAX_PAGES;

/// A folder still to be read, by its folder key.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<String, Found>;

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub key: String,
    pub name: String,
    /// The folder path this file sat in, starting at the crawled folder's name.
    pub path: String,
    pub size: Option<u64>,
}

/// Starts at the crawled folder, under its own name.
#[must_use]
pub fn start(key: &str, name: &str) -> Walk {
    Walk::start_at(key.to_owned(), join("", name))
}

/// Takes what one whole folder held; `truncated` says its chunks ran past the cap.
pub trait Absorb {
    fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>, truncated: bool);
}

impl Absorb for Walk {
    fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>, truncated: bool) {
        if truncated {
            self.note(Limit::Pages);
        }
        for entry in entries {
            match entry {
                Entry::File { key, name, size } => {
                    let found = Found {
                        key,
                        name,
                        path: folder.path.clone(),
                        size,
                    };
                    if !self.add_file(found) {
                        break;
                    }
                }
                Entry::Folder { key, name } => {
                    self.enter(folder, key, &name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Absorb, Limit, start};
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
        let mut walk = start("root", "Walls");
        let root = walk.next_folder().expect("root");
        assert_eq!((root.path.as_str(), root.depth), ("Walls", 0));
        walk.absorb(
            &root,
            vec![file("a.png"), folder("sub", "Test/Folder")],
            false,
        );
        let sub = walk.next_folder().expect("subfolder");
        assert_eq!((sub.path.as_str(), sub.depth), ("Walls/TestFolder", 1));
        walk.absorb(&sub, vec![file("b.png")], false);
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.limit(), None);
        let files = walk.into_files();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].key, "ka.png");
        assert_eq!(files[1].path, "Walls/TestFolder");
    }

    #[test]
    fn a_folder_that_contains_itself_is_read_once() {
        let mut walk = start("root", "Loop");
        let root = walk.next_folder().expect("root");
        walk.absorb(&root, vec![folder("root", "Loop"), folder("x", "X")], false);
        let x = walk.next_folder().expect("x");
        walk.absorb(&x, vec![folder("root", "Loop again")], false);
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.limit(), None);
    }

    /// A folder whose chunks ran past the cap is recorded as cut short.
    #[test]
    fn a_truncated_folder_is_named_as_the_page_limit() {
        let mut walk = start("root", "Chunked");
        let root = walk.next_folder().expect("root");
        walk.absorb(&root, vec![file("a")], true);
        assert_eq!(walk.limit(), Some(Limit::Pages));
        assert_eq!(walk.into_files().len(), 1);
    }
}

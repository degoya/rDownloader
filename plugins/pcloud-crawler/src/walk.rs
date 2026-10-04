//! Walking a pCloud folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07): what a person gets back from pasting a folder should not
//! depend on which cloud it was in. Depth, count, folders, and a set of visited ids against a
//! folder that contains itself.
//!
//! **There is deliberately no page limit here, and that is a measurement rather than an
//! omission.** RD-106-04's fourth rule added one because Google Drive, Microsoft Graph and
//! Dropbox all answer a listing a page at a time and hand back a cursor. pCloud does not:
//! `listfolder` takes no `offset`, `limit` or cursor of any kind and answers a folder whole,
//! and `showpublink` answers a public folder link with its **entire tree** in one document.
//! So there is no cursor to carry and no page to cap — this crawler never continues a folder, so
//! the shared walk's page limit never fires here; what bounds a wide answer instead is the
//! manifest's `max_response_bytes`, and what bounds a deep one is the shared `MAX_DEPTH`. A page
//! limit written here would be a limit on something that never happens, which is worse than no
//! limit at all because it would read as protection.
//!
//! The walk is keyed by `folderid` rather than by name, which is what makes it work for both
//! sources: a folder fetched from `listfolder` and a folder already sitting in the tree
//! `showpublink` answered with are the same node to it.

use crate::listing::Entry;

pub use plugin_common::walk::{Limit, join};

/// A folder still to be read, by its `folderid`.
pub type Pending = plugin_common::walk::Pending<u64>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<u64, Found>;

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    /// The folder the file sits in, which the canonical address carries.
    pub folder_id: u64,
    pub file_id: u64,
    pub name: String,
    /// The folder path this file sat in, relative to the crawled address.
    pub path: String,
    pub size: Option<u64>,
}

/// Takes what one folder held.
pub trait Absorb {
    fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>);
}

impl Absorb for Walk {
    fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>) {
        for entry in entries {
            match entry {
                Entry::Folder { folder_id, name } => {
                    self.enter(folder, folder_id, &name);
                }
                Entry::File {
                    file_id,
                    name,
                    size,
                } => {
                    self.add_file(Found {
                        folder_id: folder.id,
                        file_id,
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
    use super::{Absorb, Walk};
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

    /// A folder that contains itself is read once — by identifier, which is a stronger guard
    /// than a name and the one pCloud makes available.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = Walk::start(5);
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            walk.absorb(&pending, vec![folder(5), file(1)]);
        }
        assert_eq!(reads, 1, "a folder that contains itself is read once");
        let mut walk = Walk::start(1);
        let root = walk.next_folder().expect("the root");
        walk.absorb(&root, vec![folder(2), folder(2)]);
        assert_eq!(walk.pending().len(), 1, "the same id twice is one folder");
    }

    /// A file keeps the folder it sat in, which its canonical address is made of.
    #[test]
    fn a_file_keeps_its_folder_and_path() {
        let mut walk = Walk::start(7);
        let mut root = walk.next_folder().expect("the root");
        root.path = super::join("", "Photos");
        walk.absorb(&root, vec![folder(8), file(70)]);
        let child = walk.next_folder().expect("the child");
        walk.absorb(&child, vec![file(80)]);
        let files = walk.into_files();
        assert_eq!((files[0].folder_id, files[0].file_id), (7, 70));
        assert_eq!(files[0].path, "Photos");
        assert_eq!((files[1].folder_id, files[1].file_id), (8, 80));
        assert_eq!(files[1].path, "Photos/d8");
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

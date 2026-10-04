//! Walking a Premiumize cloud folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07); this crawler established the numbers. A crawler follows
//! addresses a stranger controls, so depth, count and cycles are somebody else's to trigger,
//! and the walk refuses rather than trusts. What is Premiumize's own is here: a folder is named
//! by its id, and a file keeps the link `folder/list` handed out for it. `folder/list` answers a
//! folder whole, so the shared page limit never fires here.

use crate::listing::Entry;

pub use plugin_common::walk::Limit;

/// A folder still to be read, by its Premiumize id.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<String, Found>;

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub url: String,
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
                Entry::Folder { id, name } => {
                    self.enter(folder, id, &name);
                }
                Entry::File { name, url, size } => {
                    self.add_file(Found {
                        url,
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

    fn folder(id: &str) -> Entry {
        Entry::Folder {
            id: id.to_owned(),
            name: format!("dir-{id}"),
        }
    }

    fn file(name: &str) -> Entry {
        Entry::File {
            name: name.to_owned(),
            url: format!("https://example.invalid/{name}"),
            size: Some(1),
        }
    }

    /// A folder that contains itself is read once. Without this the walk never ends, and
    /// every request it makes looks perfectly reasonable on its own.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = Walk::start("root".to_owned());
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            // Every folder claims to contain the root and a sibling that claims the same.
            walk.absorb(&pending, vec![folder("root"), folder("loop"), file("a")]);
        }
        assert_eq!(reads, 2, "root and `loop`, each read exactly once");
        assert_eq!(walk.limit(), None);
        let files = walk.into_files();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].url, "https://example.invalid/a");
    }

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_folder_name_stays_inside_the_path() {
        let mut walk = Walk::start("root".to_owned());
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

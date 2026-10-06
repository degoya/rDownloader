//! The listing entry, the found file and the `absorb` most crawlers share (RD-1120-10, PL-5).
//!
//! A provider that names a folder and a file by an opaque string — an item id, a file id, an
//! address — and states a name and maybe a size for each is read the same way whatever cloud it
//! is: a folder is entered under its name, a file is kept with the path of the folder it sat
//! in. Box, Google Drive and OneDrive carried this as three byte-equal copies, and the open
//! directory index as a fourth under other names. A crawler whose provider differs — a path as
//! the folder's name, numeric ids, a listing that can be cut short — keeps its own.

use super::{Pending, Walk};

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder {
        /// How the provider names the folder; the walk's cycle guard compares it.
        id: String,
        name: String,
    },
    File {
        /// How the provider names the file: its id, or its address where that is the name.
        id: String,
        name: String,
        size: Option<u64>,
    },
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

impl Walk<String, Found> {
    /// Takes what one page of one folder held: its folders are entered, its files kept.
    pub fn absorb<E: Into<Entry>>(
        &mut self,
        folder: &Pending<String>,
        entries: impl IntoIterator<Item = E>,
    ) {
        for entry in entries {
            match entry.into() {
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
    use super::{Entry, Found};
    use crate::walk::{Limit, Walk, join};

    type StandardWalk = Walk<String, Found>;

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

    /// A folder that contains itself is read once: the guard is the provider's id.
    #[test]
    fn a_cycle_is_walked_once() {
        let mut walk = StandardWalk::start("root".to_owned());
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
        let mut walk = StandardWalk::start("root".to_owned());
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

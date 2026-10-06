//! Walking an open directory tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07), and they matter more here than anywhere: an open
//! listing belongs to a stranger. A tree nested a thousand deep, a directory holding a million
//! names, and a symlink that points at its own parent — the one that cannot be noticed by "it
//! is taking a while" because every request looks perfectly reasonable — are all somebody
//! else's to trigger. What is this crawler's own is here: a directory is named by its address,
//! and the shared walk's folder limit is its directory limit. A listing page answers a
//! directory whole, so the shared page limit never fires here.

use crate::listing::Entry;

pub use plugin_common::walk::{Found, Limit, join};

/// A directory still to be read, by its address.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl. A found file's `id` is its address.
pub type Walk = plugin_common::walk::Walk<String, Found>;

/// Starts at the crawled address, with the path every file is reported under.
#[must_use]
pub fn start(url: &str, root: &str) -> Walk {
    Walk::start_at(url.to_owned(), root.to_owned())
}

/// A listing entry as the shared walk takes it (RD-1120-10): a directory is a folder named by
/// its address, and a file is kept under its address.
impl From<Entry> for plugin_common::walk::Entry {
    fn from(entry: Entry) -> Self {
        match entry {
            Entry::Directory { url, name } => Self::Folder { id: url, name },
            Entry::File { url, name, size } => Self::File {
                id: url,
                name,
                size,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Walk, join};
    use crate::listing::Entry;
    use plugin_common::walk::MAX_FOLDERS;

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
        super::start("https://files.example.org/pub/", "pub")
    }

    /// A symlink that points back at a directory already read is read once. Without this
    /// the walk never ends, and every request it makes looks perfectly reasonable.
    #[test]
    fn a_loop_of_symlinks_is_walked_once_and_not_for_ever() {
        let mut walk = start();
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            assert!(reads <= MAX_FOLDERS, "the walk did not terminate");
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

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_directory_name_stays_inside_the_path() {
        assert_eq!(join("pub", "../../etc"), "pub/etc");
        assert_eq!(join("pub", "  "), "pub");
        assert_eq!(join("", "Season 1"), "Season 1");
        assert_eq!(join("pub", "a\\b"), "pub/ab");
    }
}

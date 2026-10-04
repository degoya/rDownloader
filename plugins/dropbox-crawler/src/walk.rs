//! Walking a folder tree under limits it cannot talk its way out of — and carrying the cursor.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07): what a person gets back from pasting a folder should not
//! depend on which cloud it was in. Depth, count, folders, and a
//! set of visited paths against a folder that contains itself — compared without regard to
//! case, because Dropbox paths are case-insensitive ([`ApiPath`]). Plus the one a paginating API
//! needs: **pages**. Dropbox answers `list_folder` a page at a time and hands back a `cursor`,
//! so a folder wide enough produces pages for as long as anybody asks.
//!
//! What is different here is *where the cursor lives*. It is not a local of the loop that
//! reads one folder: it is part of the walk's own state, in the [`Pending`] entry for that
//! folder (`Walk::continue_folder`), which is re-queued at the front with the cursor Dropbox handed back. A crawl is
//! therefore one queue of "folders and where I was in each", and any page can be resumed from
//! exactly that entry — the cursor survives whatever happens between two pages, and a walk
//! interrupted and rebuilt from its pending entries continues where it stopped rather than
//! from page one. The fuel and time budget in `manifest.toml` sit underneath as the last
//! resort; they stop a runaway, they do not bound a correct crawl.

use crate::listing::Entry;

pub use plugin_common::walk::{Limit, join};

/// A folder's Dropbox API path, spelled as Dropbox spelled it.
///
/// Requests carry the original spelling; the walk's cycle guard compares two paths without
/// regard to ASCII case, because Dropbox treats `/Show/Extras` and `/show/extras` as one folder
/// and a guard that did not would read it twice.
#[derive(Clone, Debug, Eq)]
pub struct ApiPath(pub String);

impl ApiPath {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for ApiPath {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

/// A folder still to be read — or the next page of one, under the cursor Dropbox handed back.
pub type Pending = plugin_common::walk::Pending<ApiPath>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<ApiPath, Found>;

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    /// The API path of the folder the file sits in, which the canonical address carries.
    pub folder: String,
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
                Entry::Folder { name } => {
                    let child = ApiPath(format!("{}/{name}", folder.id.as_str()));
                    self.enter(folder, child, &name);
                }
                Entry::File { name, size } => {
                    self.add_file(Found {
                        folder: folder.id.as_str().to_owned(),
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
    use super::{Absorb, ApiPath, Limit, Walk};
    use crate::listing::Entry;
    use plugin_common::walk::{MAX_DEPTH, MAX_FOLDERS};

    fn start(path: &str) -> Walk {
        Walk::start(ApiPath(path.to_owned()))
    }

    fn folder(name: &str) -> Entry {
        Entry::Folder {
            name: name.to_owned(),
        }
    }

    fn file(name: &str) -> Entry {
        Entry::File {
            name: name.to_owned(),
            size: Some(1),
        }
    }

    /// A folder that contains itself — by name, whatever the case — is read once.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = start("/Loop");
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            assert!(reads <= MAX_FOLDERS, "the walk did not terminate");
            walk.absorb(&pending, vec![folder("loop"), file("a")]);
        }
        // `/Loop` and `/Loop/loop`, then `/Loop/loop/loop` ... until depth stops it — but never
        // the same path twice.
        assert_eq!(reads as u32, MAX_DEPTH + 1);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        let mut walk = start("/Show");
        let root = walk.next_folder().expect("the root");
        walk.absorb(&root, vec![folder("Extras"), folder("extras")]);
        assert_eq!(
            walk.pending().len(),
            1,
            "the same folder in another case is one folder"
        );
        // The request keeps the spelling Dropbox listed first.
        assert_eq!(walk.pending()[0].id.as_str(), "/Show/Extras");
    }

    /// The cursor is walk state: a folder with more pages comes back as the next thing to read,
    /// carrying the cursor its last page ended on, and a page of it does not count as a folder.
    #[test]
    fn a_folder_with_more_pages_is_continued_from_its_cursor_before_anything_else() {
        let mut walk = start("/Show");
        let root = walk.next_folder().expect("the root");
        walk.absorb(&root, vec![folder("Season 1"), file("a")]);
        walk.continue_folder(&root, "cursor-1".to_owned());

        let next = walk.next_folder().expect("the continuation");
        assert_eq!(next.id.as_str(), "/Show");
        assert_eq!(next.cursor.as_deref(), Some("cursor-1"));
        assert_eq!(next.page, 1);
        assert_eq!(next.path, root.path);
        assert_eq!(walk.folders_read(), 1, "a page is not a second folder");
        // The sub-folder waits behind the continuation.
        walk.absorb(&next, vec![file("b")]);
        let child = walk.next_folder().expect("the sub-folder");
        assert_eq!(child.id.as_str(), "/Show/Season 1");
        assert_eq!(child.cursor, None);
        assert_eq!(walk.folders_read(), 2);
        let files = walk.into_files();
        assert_eq!(files[0].folder, "/Show");
        assert_eq!(files[1].folder, "/Show");
    }

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_folder_name_stays_inside_the_path() {
        let mut walk = start("");
        let pending = walk.next_folder().expect("the root");
        walk.absorb(
            &pending,
            vec![folder("..etc"), folder("  "), folder("a\u{0}b")],
        );
        assert_eq!(walk.next_folder().expect("first child").path, "etc");
        assert_eq!(walk.next_folder().expect("second child").path, "");
        assert_eq!(walk.next_folder().expect("third child").path, "ab");
    }
}

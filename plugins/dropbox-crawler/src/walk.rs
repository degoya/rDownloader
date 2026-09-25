//! Walking a folder tree under limits it cannot talk its way out of — and carrying the cursor.
//!
//! The four limits are the ones `plugins/premiumize-crawler/src/walk.rs` established and
//! `plugins/google-drive-crawler` kept, deliberately unchanged: what a person gets back from
//! pasting a folder should not depend on which cloud it was in. Depth, count, folders, and a
//! set of visited paths against a folder that contains itself. Plus the one a paginating API
//! needs: **pages**. Dropbox answers `list_folder` a page at a time and hands back a `cursor`,
//! so a folder wide enough produces pages for as long as anybody asks.
//!
//! What is different here is *where the cursor lives*. It is not a local of the loop that
//! reads one folder: it is part of the walk's own state, in the [`Pending`] entry for that
//! folder, which is re-queued at the front with the cursor Dropbox handed back. A crawl is
//! therefore one queue of "folders and where I was in each", and any page can be resumed from
//! exactly that entry — the cursor survives whatever happens between two pages, and a walk
//! interrupted and rebuilt from its pending entries continues where it stopped rather than
//! from page one. The fuel and time budget in `manifest.toml` sit underneath as the last
//! resort; they stop a runaway, they do not bound a correct crawl.

use crate::listing::Entry;

/// How many levels below the crawled address are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most folders one crawl reads, including the one it was given.
pub const MAX_FOLDERS: usize = 100;
/// Most pages of one folder's listing that are followed.
pub const MAX_PAGES: usize = 10;

/// A folder still to be read, or a folder read up to a cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    /// The folder as the API names it: absolute in the account's own Dropbox, relative to the
    /// link's root inside a shared folder link; empty for the root either way.
    pub api_path: String,
    /// Path relative to the crawled address, for the package suggestion; empty for the address
    /// itself.
    pub path: String,
    pub depth: u32,
    /// Where the previous page of this folder stopped. `None` for a folder not yet read.
    pub cursor: Option<String>,
    /// Which page of this folder the cursor asks for; zero for the first.
    pub page: usize,
}

/// One file the walk found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    /// The folder this file sat in, as the API names it.
    pub folder: String,
    pub name: String,
    /// The folder path this file sat in, relative to the crawled address.
    pub path: String,
    pub size: Option<u64>,
}

/// Which limit stopped the walk short, when one did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    Depth,
    Files,
    Folders,
    Pages,
}

/// The state of one crawl: the folders still to read, each with the cursor it was read up to.
pub struct Walk {
    queue: Vec<Pending>,
    /// Every folder ever enqueued, by its API path in lower case — Dropbox paths are
    /// case-insensitive. A folder reached twice is read once; this is the cycle guard.
    seen: Vec<String>,
    files: Vec<Found>,
    read: usize,
    limit: Option<Limit>,
}

impl Walk {
    /// Starts at the folder the crawled address named.
    #[must_use]
    pub fn start(root: &str) -> Self {
        Self {
            queue: vec![Pending {
                api_path: root.to_owned(),
                path: String::new(),
                depth: 0,
                cursor: None,
                page: 0,
            }],
            seen: vec![root.to_ascii_lowercase()],
            files: Vec::new(),
            read: 0,
            limit: None,
        }
    }

    /// The next folder to fetch — or the next page of one — or `None` when the walk is over.
    pub fn next_folder(&mut self) -> Option<Pending> {
        if self.is_full() {
            self.limit.get_or_insert(Limit::Files);
            return None;
        }
        // A continuation is the same folder, so it does not count against the folder limit.
        let continuing = self.queue.first().is_some_and(|next| next.cursor.is_some());
        if !continuing && self.read >= MAX_FOLDERS {
            self.limit.get_or_insert(Limit::Folders);
            return None;
        }
        // Breadth first: the files nearest the address a person pasted are the ones they meant,
        // so if a limit does cut the walk short it cuts the far end off.
        if self.queue.is_empty() {
            return None;
        }
        if !continuing {
            self.read += 1;
        }
        Some(self.queue.remove(0))
    }

    /// Whether the file limit has been reached, which is what stops one folder's pagination as
    /// well as the walk itself.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.files.len() >= MAX_FILES
    }

    /// Records a limit the caller hit.
    pub fn note(&mut self, limit: Limit) {
        self.limit.get_or_insert(limit);
    }

    /// Puts a folder back at the front of the queue with the cursor its last page ended on, so
    /// the next call to [`Self::next_folder`] continues it — or records that it pages further
    /// than the cap and leaves it.
    ///
    /// A full walk stops paging too: the files past the limit are not going to be handed back,
    /// so fetching them costs requests for nothing.
    pub fn continue_folder(&mut self, folder: &Pending, cursor: String) {
        if self.is_full() {
            self.limit.get_or_insert(Limit::Files);
            return;
        }
        if folder.page + 1 >= MAX_PAGES {
            self.limit.get_or_insert(Limit::Pages);
            return;
        }
        self.queue.insert(
            0,
            Pending {
                api_path: folder.api_path.clone(),
                path: folder.path.clone(),
                depth: folder.depth,
                cursor: Some(cursor),
                page: folder.page + 1,
            },
        );
    }

    /// Takes what one page of one folder held.
    pub fn absorb(&mut self, folder: &Pending, entries: Vec<Entry>) {
        for entry in entries {
            match entry {
                Entry::Folder { name } => {
                    if folder.depth + 1 > MAX_DEPTH {
                        self.limit.get_or_insert(Limit::Depth);
                        continue;
                    }
                    let api_path = format!("{}/{name}", folder.api_path);
                    let key = api_path.to_ascii_lowercase();
                    if self.seen.iter().any(|known| known == &key) {
                        continue;
                    }
                    self.seen.push(key);
                    self.queue.push(Pending {
                        api_path,
                        path: join(&folder.path, &name),
                        depth: folder.depth + 1,
                        cursor: None,
                        page: 0,
                    });
                }
                Entry::File { name, size } => {
                    if self.is_full() {
                        self.limit.get_or_insert(Limit::Files);
                        continue;
                    }
                    self.files.push(Found {
                        folder: folder.api_path.clone(),
                        name,
                        path: folder.path.clone(),
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

    /// How many folders were read.
    #[must_use]
    pub const fn folders_read(&self) -> usize {
        self.read
    }

    /// The folders — and pages — still to read, which is the whole of what a resumed walk
    /// needs.
    #[must_use]
    pub fn pending(&self) -> &[Pending] {
        &self.queue
    }

    /// What the walk found.
    #[must_use]
    pub fn into_files(self) -> Vec<Found> {
        self.files
    }
}

/// Joins a folder path with a child name, keeping the result a relative path.
pub fn join(path: &str, name: &str) -> String {
    // A name a stranger chose must not be able to leave the path it belongs to, so the two
    // characters that would let it are dropped rather than escaped.
    let safe: String = name
        .chars()
        .filter(|character| !matches!(character, '/' | '\\') && !character.is_control())
        .collect();
    let safe = safe.trim().trim_matches('.').trim();
    if safe.is_empty() {
        return path.to_owned();
    }
    if path.is_empty() {
        return safe.chars().take(120).collect();
    }
    format!("{path}/{}", safe.chars().take(120).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::{Limit, MAX_DEPTH, MAX_FILES, MAX_FOLDERS, MAX_PAGES, Walk};
    use crate::listing::Entry;

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

    /// A tree deeper than the limit is cut at the limit, not walked to the bottom.
    #[test]
    fn the_depth_limit_stops_the_descent() {
        let mut walk = Walk::start("");
        let mut depth = 0;
        while let Some(pending) = walk.next_folder() {
            depth = pending.depth;
            walk.absorb(
                &pending,
                vec![folder(&format!("d{}", pending.depth + 1)), file("a")],
            );
        }
        assert_eq!(depth, MAX_DEPTH);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        assert_eq!(walk.into_files().len() as u32, MAX_DEPTH + 1);
    }

    /// A folder holding more files than the limit hands back the limit, and says so.
    #[test]
    fn the_count_limit_stops_the_listing() {
        let mut walk = Walk::start("");
        let pending = walk.next_folder().expect("the root");
        let entries: Vec<Entry> = (0..MAX_FILES + 50)
            .map(|index| file(&format!("f{index}")))
            .collect();
        walk.absorb(&pending, entries);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert!(walk.is_full());
        // A full walk stops paginating as well as walking.
        walk.continue_folder(&pending, "cursor".to_owned());
        assert!(walk.pending().is_empty());
        assert!(walk.next_folder().is_none());
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// A folder that contains itself — by name, whatever the case — is read once.
    #[test]
    fn a_cycle_is_walked_once_and_not_for_ever() {
        let mut walk = Walk::start("/Loop");
        let mut reads = 0;
        while let Some(pending) = walk.next_folder() {
            reads += 1;
            assert!(reads <= MAX_FOLDERS, "the walk did not terminate");
            walk.absorb(&pending, vec![folder("loop"), file("a")]);
        }
        // `/Loop` and `/Loop/loop`, then `/Loop/loop/loop` … until depth stops it — but never
        // the same path twice.
        assert_eq!(reads as u32, MAX_DEPTH + 1);
        let mut walk = Walk::start("/Show");
        let root = walk.next_folder().expect("the root");
        walk.absorb(&root, vec![folder("Extras"), folder("extras")]);
        assert_eq!(
            walk.pending().len(),
            1,
            "the same folder in another case is one folder"
        );
    }

    /// A folder wider than the folder limit stops at it rather than reading for ever.
    #[test]
    fn the_folder_limit_stops_the_walk() {
        let mut walk = Walk::start("");
        let root = walk.next_folder().expect("the root");
        let children: Vec<Entry> = (0..MAX_FOLDERS + 50)
            .map(|index| folder(&format!("n{index}")))
            .collect();
        walk.absorb(&root, children);
        while let Some(pending) = walk.next_folder() {
            walk.absorb(&pending, vec![file("a")]);
        }
        assert_eq!(walk.folders_read(), MAX_FOLDERS);
        assert_eq!(walk.limit(), Some(Limit::Folders));
    }

    /// The cursor is walk state: a folder with more pages comes back as the next thing to read,
    /// carrying the cursor its last page ended on, and a page of it does not count as a folder.
    #[test]
    fn a_folder_with_more_pages_is_continued_from_its_cursor_before_anything_else() {
        let mut walk = Walk::start("/Show");
        let root = walk.next_folder().expect("the root");
        walk.absorb(&root, vec![folder("Season 1"), file("a")]);
        walk.continue_folder(&root, "cursor-1".to_owned());

        let next = walk.next_folder().expect("the continuation");
        assert_eq!(next.api_path, "/Show");
        assert_eq!(next.cursor.as_deref(), Some("cursor-1"));
        assert_eq!(next.page, 1);
        assert_eq!(next.path, root.path);
        assert_eq!(walk.folders_read(), 1, "a page is not a second folder");
        // The sub-folder waits behind the continuation.
        walk.absorb(&next, vec![file("b")]);
        let child = walk.next_folder().expect("the sub-folder");
        assert_eq!(child.api_path, "/Show/Season 1");
        assert_eq!(child.cursor, None);
        assert_eq!(walk.folders_read(), 2);
    }

    /// A folder that pages further than the cap is recorded as cut short rather than followed
    /// for as long as Dropbox hands out cursors — and the first limit hit is the one reported.
    #[test]
    fn a_folder_that_pages_further_than_the_cap_is_recorded_as_cut_short() {
        let mut walk = Walk::start("");
        let mut pending = walk.next_folder().expect("the root");
        let mut pages = 1;
        loop {
            walk.absorb(&pending, vec![file(&format!("f{pages}"))]);
            walk.continue_folder(&pending, format!("cursor-{pages}"));
            match walk.next_folder() {
                Some(next) => {
                    pending = next;
                    pages += 1;
                }
                None => break,
            }
        }
        assert_eq!(pages, MAX_PAGES);
        assert_eq!(walk.limit(), Some(Limit::Pages));
        walk.note(Limit::Depth);
        assert_eq!(walk.limit(), Some(Limit::Pages));
        assert_eq!(walk.into_files().len(), MAX_PAGES);
    }

    /// A name a stranger chose cannot climb out of the path it belongs to.
    #[test]
    fn a_folder_name_stays_inside_the_path() {
        let mut walk = Walk::start("");
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

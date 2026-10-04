//! Walking a folder tree under limits it cannot talk its way out of, once (RD-191-07, PLUG-11).
//!
//! The four limits are not politeness. A crawler follows addresses a stranger controls, so
//! every failure mode below is somebody else's to trigger:
//!
//! - **Depth** — a tree nested a thousand deep costs a request per level.
//! - **Count** — a folder holding a million entries fills the review list with them.
//! - **Pages** — a provider that answers a listing a page at a time produces pages for as long
//!   as anybody asks, so a folder is continued a bounded number of times and no further.
//! - **Cycles** — a shortcut can point at an ancestor, so a folder can contain itself through
//!   three others. This is the one that cannot be caught by "it is taking a while": every
//!   single request looks perfectly reasonable.
//!
//! So the walk carries its own bookkeeping and refuses rather than trusts. The fuel and time
//! budget in a plugin's `manifest.toml` sit underneath as the last resort; they stop a runaway,
//! they do not bound a correct crawl, and a plugin that relied on them would report a timeout
//! where it should report "this folder is bigger than I will list".
//!
//! Nine crawlers carried this walk as nine copies with the same numbers — the ones
//! `premiumize-crawler` established and `google-drive-crawler` extended by the page cap — and
//! a limit reimplemented per provider is a limit that drifts. What a person gets back from
//! pasting a folder should not depend on which cloud it was in. What stays in a crawler is what
//! differs: how a folder is named (`Id`), what it keeps of a file (`File`), and how one page of
//! its provider's listing is read.
//!
//! Plain Rust with no dependencies, so a guest that takes it gains no import.

/// How many levels below the crawled address are walked.
pub const MAX_DEPTH: u32 = 4;
/// Most files one crawl hands back.
pub const MAX_FILES: usize = 500;
/// Most folders one crawl reads, including the one it was given.
pub const MAX_FOLDERS: usize = 100;
/// Most pages of one folder's listing that are followed.
pub const MAX_PAGES: usize = 10;
/// Longest folder name, in characters, that becomes one segment of a path.
const MAX_SEGMENT_CHARS: usize = 120;

/// A folder still to be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending<Id> {
    /// How the provider names the folder: an item id, an API path, an address.
    pub id: Id,
    /// Path relative to the crawled address; empty for the address itself unless the crawler
    /// started the walk under the crawled folder's own name.
    pub path: String,
    pub depth: u32,
    /// The provider's continuation token, when this is a later page of a folder already read.
    pub cursor: Option<String>,
    /// Which page of the folder this is; `0` for the first.
    pub page: usize,
}

/// Which limit stopped the walk short, when one did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    Depth,
    Files,
    Folders,
    Pages,
}

/// The state of one crawl: folders by `Id`, files as whatever the crawler keeps of one.
///
/// `Id`'s `PartialEq` is the cycle guard. A provider whose names are case-insensitive — a
/// Dropbox path — gives its `Id` an equality that says so; one whose names need normalising
/// — a WebDAV `href` — normalises before it hands one over.
pub struct Walk<Id, File> {
    queue: Vec<Pending<Id>>,
    /// Every folder ever enqueued. A folder reached twice is read once — this is the cycle
    /// guard, and it is by id rather than by path because a cycle changes the path every time
    /// round and would otherwise look like new ground for ever.
    seen: Vec<Id>,
    files: Vec<File>,
    read: usize,
    limit: Option<Limit>,
}

impl<Id: Clone + PartialEq, File> Walk<Id, File> {
    /// Starts at the folder the crawl was given, with an empty path.
    #[must_use]
    pub fn start(root: Id) -> Self {
        Self::start_at(root, String::new())
    }

    /// Starts at the folder the crawl was given, with the path its files are reported under —
    /// the crawled folder's own name, for a crawler that roots its package hints in it.
    #[must_use]
    pub fn start_at(root: Id, path: String) -> Self {
        Self {
            queue: vec![Pending {
                id: root.clone(),
                path,
                depth: 0,
                cursor: None,
                page: 0,
            }],
            seen: vec![root],
            files: Vec::new(),
            read: 0,
            limit: None,
        }
    }

    /// The next folder — or the next page of one — to fetch, or `None` when the walk is over.
    pub fn next_folder(&mut self) -> Option<Pending<Id>> {
        // Nothing left is the end of a walk, not a limit: a tree of exactly a hundred folders,
        // or of exactly five hundred files, was read whole and must not be reported as cut.
        let next = self.queue.first()?;
        // A continuation is the same folder, so it does not count against the folder limit.
        let continuing = next.cursor.is_some();
        if self.is_full() {
            self.limit.get_or_insert(Limit::Files);
            return None;
        }
        if !continuing && self.read >= MAX_FOLDERS {
            self.limit.get_or_insert(Limit::Folders);
            return None;
        }
        // Breadth first: the files nearest the address a person pasted are the ones they meant,
        // so if a limit does cut the walk short it cuts the far end off.
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

    /// Records a limit the caller hit — a page cap only the caller can see, say.
    pub fn note(&mut self, limit: Limit) {
        self.limit.get_or_insert(limit);
    }

    /// Whether `folder` may be read for one more page. `false` records the page limit, so a
    /// crawler that paginates in its own loop asks this before every page after the first.
    pub fn another_page(&mut self, pages_read: usize) -> bool {
        if pages_read >= MAX_PAGES {
            self.limit.get_or_insert(Limit::Pages);
            return false;
        }
        true
    }

    /// Queues the next page of `folder` ahead of everything else, under the provider's cursor.
    /// Past [`MAX_PAGES`] — or once the walk is full — the folder is cut short instead, and the
    /// limit says so.
    pub fn continue_folder(&mut self, folder: &Pending<Id>, cursor: String) {
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
                id: folder.id.clone(),
                path: folder.path.clone(),
                depth: folder.depth,
                cursor: Some(cursor),
                page: folder.page + 1,
            },
        );
    }

    /// Takes one subfolder `folder` listed. `false` when it is not queued: below the depth
    /// limit (which is recorded), or already seen.
    pub fn enter(&mut self, folder: &Pending<Id>, id: Id, name: &str) -> bool {
        if folder.depth + 1 > MAX_DEPTH {
            self.limit.get_or_insert(Limit::Depth);
            return false;
        }
        if self.seen.contains(&id) {
            return false;
        }
        self.seen.push(id.clone());
        self.queue.push(Pending {
            id,
            path: join(&folder.path, name),
            depth: folder.depth + 1,
            cursor: None,
            page: 0,
        });
        true
    }

    /// Takes one file. `false` when the walk is full, which is recorded.
    pub fn add_file(&mut self, file: File) -> bool {
        if self.is_full() {
            self.limit.get_or_insert(Limit::Files);
            return false;
        }
        self.files.push(file);
        true
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

    /// The folders still queued, next first.
    #[must_use]
    pub fn pending(&self) -> &[Pending<Id>] {
        &self.queue
    }

    /// What the walk has found so far.
    #[must_use]
    pub fn files(&self) -> &[File] {
        &self.files
    }

    /// What the walk found.
    #[must_use]
    pub fn into_files(self) -> Vec<File> {
        self.files
    }
}

/// Joins a folder path with a child name, keeping the result a relative path.
#[must_use]
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
    let segment: String = safe.chars().take(MAX_SEGMENT_CHARS).collect();
    if path.is_empty() {
        return segment;
    }
    format!("{path}/{segment}")
}

#[cfg(test)]
mod tests {
    use super::{Limit, MAX_DEPTH, MAX_FILES, MAX_FOLDERS, MAX_PAGES, Pending, Walk, join};

    type TestWalk = Walk<String, String>;

    fn absorb(walk: &mut TestWalk, folder: &Pending<String>, folders: &[&str], files: &[&str]) {
        for id in folders {
            walk.enter(folder, (*id).to_owned(), &format!("dir-{id}"));
        }
        for name in files {
            walk.add_file((*name).to_owned());
        }
    }

    /// A tree deeper than the limit is cut at the limit, not walked to the bottom.
    #[test]
    fn the_depth_limit_stops_the_descent() {
        let mut walk = TestWalk::start("root".to_owned());
        let mut depth = 0;
        while let Some(pending) = walk.next_folder() {
            depth = pending.depth;
            let child = format!("d{}", pending.depth + 1);
            absorb(&mut walk, &pending, &[&child], &["a"]);
        }
        assert_eq!(depth, MAX_DEPTH);
        assert_eq!(walk.limit(), Some(Limit::Depth));
        assert_eq!(walk.folders_read() as u32, MAX_DEPTH + 1);
    }

    /// A folder with more files than the limit hands back the limit and says so.
    #[test]
    fn the_file_limit_stops_the_walk_and_is_reported() {
        let mut walk = TestWalk::start("root".to_owned());
        let pending = walk.next_folder().expect("root");
        for index in 0..MAX_FILES + 50 {
            walk.add_file(format!("f{index}"));
        }
        assert!(walk.is_full());
        assert_eq!(walk.next_folder(), None);
        assert_eq!(walk.limit(), Some(Limit::Files));
        assert!(!walk.add_file("late".to_owned()));
        walk.continue_folder(&pending, "cursor".to_owned());
        assert!(walk.pending().is_empty());
        assert_eq!(walk.into_files().len(), MAX_FILES);
    }

    /// More subfolders than the limit are read up to the limit, breadth first.
    #[test]
    fn the_folder_limit_stops_the_walk_and_is_reported() {
        let mut walk = TestWalk::start("root".to_owned());
        let pending = walk.next_folder().expect("root");
        for index in 0..MAX_FOLDERS + 20 {
            walk.enter(&pending, format!("s{index}"), "sub");
        }
        let mut read = 1;
        while let Some(pending) = walk.next_folder() {
            read += 1;
            walk.add_file(format!("in-{}", pending.id));
        }
        assert_eq!(read, MAX_FOLDERS);
        assert_eq!(walk.limit(), Some(Limit::Folders));
    }

    /// A tree that fits exactly is read whole, and nothing is reported as cut.
    #[test]
    fn a_tree_that_fits_exactly_reports_no_limit() {
        let mut walk = TestWalk::start("root".to_owned());
        let root = walk.next_folder().expect("root");
        for index in 1..MAX_FOLDERS {
            walk.enter(&root, format!("s{index}"), "sub");
        }
        while let Some(pending) = walk.next_folder() {
            if walk.files().len() < MAX_FILES {
                walk.add_file(format!("in-{}", pending.id));
            }
        }
        assert_eq!(walk.folders_read(), MAX_FOLDERS);
        assert_eq!(walk.limit(), None);

        let mut full = TestWalk::start("root".to_owned());
        full.next_folder().expect("root");
        for index in 0..MAX_FILES {
            full.add_file(format!("f{index}"));
        }
        assert_eq!(full.next_folder(), None);
        assert_eq!(full.limit(), None);
    }

    /// A folder that contains itself is read once.
    #[test]
    fn a_cycle_is_walked_once() {
        let mut walk = TestWalk::start("root".to_owned());
        let root = walk.next_folder().expect("root");
        absorb(&mut walk, &root, &["inner"], &["a"]);
        let inner = walk.next_folder().expect("inner");
        assert!(!walk.enter(&inner, "root".to_owned(), "again"));
        assert!(!walk.enter(&inner, "inner".to_owned(), "self"));
        walk.add_file("b".to_owned());
        assert_eq!(walk.next_folder(), None);
        assert_eq!(walk.folders_read(), 2);
        assert_eq!(walk.limit(), None);
        assert_eq!(walk.into_files().len(), 2);
    }

    /// A continuation is read before anything else and does not count as another folder; past
    /// the page cap the folder is cut short and the walk says so.
    #[test]
    fn a_folder_is_continued_first_and_only_up_to_the_page_cap() {
        let mut walk = TestWalk::start("root".to_owned());
        let mut folder = walk.next_folder().expect("root");
        walk.enter(&folder, "sibling".to_owned(), "sibling");
        let mut pages = 1;
        loop {
            walk.continue_folder(&folder, format!("page-{pages}"));
            let next = walk.next_folder().expect("a page or the sibling");
            if next.id != "root" {
                break;
            }
            assert_eq!(
                next.cursor.as_deref(),
                Some(format!("page-{pages}").as_str())
            );
            folder = next;
            pages += 1;
        }
        assert_eq!(pages, MAX_PAGES);
        assert_eq!(walk.limit(), Some(Limit::Pages));
        assert_eq!(walk.folders_read(), 2);
    }

    #[test]
    fn a_crawler_paginating_in_its_own_loop_is_stopped_at_the_cap() {
        let mut walk = TestWalk::start("root".to_owned());
        assert!(walk.another_page(MAX_PAGES - 1));
        assert_eq!(walk.limit(), None);
        assert!(!walk.another_page(MAX_PAGES));
        assert_eq!(walk.limit(), Some(Limit::Pages));
    }

    /// The path a file is reported under is rooted in the crawled folder's own name, and a
    /// stranger's name cannot leave it.
    #[test]
    fn the_package_hint_is_the_path_below_the_crawled_folder() {
        let mut walk = TestWalk::start_at("root".to_owned(), join("", "Show"));
        let root = walk.next_folder().expect("root");
        walk.enter(&root, "s1".to_owned(), "../Season 1/");
        let season = walk.next_folder().expect("season");
        assert_eq!(season.path, "Show/Season 1");
    }

    #[test]
    fn a_name_that_is_nothing_but_separators_adds_no_segment() {
        assert_eq!(join("Show", "///"), "Show");
        assert_eq!(join("", "..."), "");
        assert_eq!(join("a", "b\u{0}c"), "a/bc");
        assert_eq!(join("", &"x".repeat(200)).len(), 120);
    }
}

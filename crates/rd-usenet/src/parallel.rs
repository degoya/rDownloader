//! How many NZB files the runner works on at once (RD-130-22).
//!
//! The connections are shared across files already: every file asks for up to the pool's
//! whole request window, and the pool's semaphore caps all of them together (RD-108-26). What
//! the file count decides is whether there is always *enough* asked for. A file near its end
//! has fewer articles left than the window has places, and a file that has not started yet
//! asks for nothing; with a fixed two, a release of small files leaves connections waiting at
//! every file boundary.
//!
//! Automatic mode therefore follows the work that is still open instead of a constant: it
//! lets another file start while the running ones together hold fewer unanswered articles
//! than two request windows. One window is what the connections carry right now; the second
//! is the head start the next file needs, because a file does not ask for anything until the
//! scheduler has dispatched it (every 500 ms) and it has read its segments, checked what a
//! resumed `.part` file already holds and opened its staging file. For files of `a` articles
//! and a window of `w`, that settles at about `ceil(2w / a)` files running - two for files
//! larger than the window, up to [`MAX_PARALLEL_FILES`] for releases of tiny ones.

use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

/// The most files the runner works on at once, automatic or set by hand.
///
/// Every running file holds an open `.part` file, writes at its own place on the disk -
/// which a spinning disk or a NAS pays for with a seek per switch - and can hold up to a
/// window of decoded articles in memory while its assembly catches up. Eight files fill a
/// window of 64 requests (32 connections, pipelined twice, the most a server allows) with
/// files of just eight articles each; a release whose files are smaller than that is so
/// small that the boundaries cost less than the seeks would.
pub(crate) const MAX_PARALLEL_FILES: usize = rd_scheduler::MAX_EXTERNAL_PARALLEL_FILES;

/// The fewest files automatic mode lets run: the tail of one file overlaps the head of the
/// next (RD-108-26), whatever the sizes.
pub(crate) const MIN_AUTO_FILES: usize = 2;

/// Files automatic mode allows at once, given what the running ones still have to fetch.
///
/// `running` files hold `open_articles` articles not answered yet; `window` is the pool's
/// request places - the primary server's connections times the pipeline depth in force
/// ([`crate::NntpPool::max_parallel_requests`]), `0` while no pool exists. Another file is
/// allowed while the open articles are fewer than two windows (see the module documentation
/// for why two), never fewer than [`MIN_AUTO_FILES`] and never more than
/// [`MAX_PARALLEL_FILES`].
#[must_use]
pub(crate) fn auto_parallel_files(running: usize, open_articles: usize, window: usize) -> usize {
    if running < MIN_AUTO_FILES {
        return MIN_AUTO_FILES;
    }
    // No pool yet means no window to fill; the floor has already been granted.
    let starved = window > 0 && open_articles < window.saturating_mul(2);
    let wanted = if starved { running + 1 } else { running };
    wanted.clamp(MIN_AUTO_FILES, MAX_PARALLEL_FILES)
}

/// What the runner's files hold open right now, shared between the runner and every file.
#[derive(Default)]
pub(crate) struct FileLoad {
    running: AtomicUsize,
    open_articles: AtomicUsize,
    /// The request window of the pool the last file was given.
    window: AtomicUsize,
    /// Time the assembly spent waiting for the database writer, in nanoseconds, and the
    /// batches it wrote. Read by the throughput bench; costs two clock reads per batch.
    writer_wait_nanos: AtomicU64,
    writer_batches: AtomicU64,
    articles_confirmed: AtomicU64,
}

impl FileLoad {
    /// Files allowed at once: `requested` when the operator fixed a number, otherwise
    /// [`auto_parallel_files`] on the current load.
    pub(crate) fn capacity(&self, requested: usize) -> usize {
        if requested > 0 {
            return requested.min(MAX_PARALLEL_FILES);
        }
        auto_parallel_files(
            self.running.load(Ordering::Acquire),
            self.open_articles.load(Ordering::Acquire),
            self.window.load(Ordering::Acquire),
        )
    }

    pub(crate) fn set_window(&self, window: usize) {
        self.window.store(window, Ordering::Release);
    }

    /// Registers a running file with `articles` still to fetch; the returned handle takes
    /// the file off the books when it is dropped, however the file ends.
    pub(crate) fn start(self: &Arc<Self>, articles: usize) -> OpenArticles {
        self.running.fetch_add(1, Ordering::AcqRel);
        self.open_articles.fetch_add(articles, Ordering::AcqRel);
        OpenArticles {
            load: Some(Arc::clone(self)),
            left: AtomicUsize::new(articles),
        }
    }

    /// Nanoseconds spent waiting for the writer, batches written and articles confirmed,
    /// since the runner started.
    pub(crate) fn writer_totals(&self) -> (u64, u64, u64) {
        (
            self.writer_wait_nanos.load(Ordering::Acquire),
            self.writer_batches.load(Ordering::Acquire),
            self.articles_confirmed.load(Ordering::Acquire),
        )
    }

    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        self.running.load(Ordering::Acquire)
    }
}

/// One running file's share of the [`FileLoad`].
pub(crate) struct OpenArticles {
    /// `None` for a file nobody counts - a direct call outside the runner.
    load: Option<Arc<FileLoad>>,
    left: AtomicUsize,
}

impl OpenArticles {
    /// A handle that counts nothing.
    #[cfg(test)]
    pub(crate) fn untracked() -> Self {
        Self {
            load: None,
            left: AtomicUsize::new(0),
        }
    }

    /// One article answered - delivered, missing or failed - so it is no longer open.
    pub(crate) fn answered(&self) {
        let taken = self
            .left
            .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if taken && let Some(load) = &self.load {
            load.open_articles.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// A batch of `articles` confirmations that waited `waited` for the writer.
    pub(crate) fn wrote(&self, articles: usize, waited: std::time::Duration) {
        if let Some(load) = &self.load {
            let nanos = u64::try_from(waited.as_nanos()).unwrap_or(u64::MAX);
            load.writer_wait_nanos.fetch_add(nanos, Ordering::AcqRel);
            load.writer_batches.fetch_add(1, Ordering::AcqRel);
            load.articles_confirmed
                .fetch_add(articles as u64, Ordering::AcqRel);
        }
    }
}

impl Drop for OpenArticles {
    fn drop(&mut self) {
        if let Some(load) = &self.load {
            load.open_articles
                .fetch_sub(*self.left.get_mut(), Ordering::AcqRel);
            load.running.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{FileLoad, MAX_PARALLEL_FILES, MIN_AUTO_FILES, auto_parallel_files};

    #[test]
    fn two_files_run_whatever_the_load() {
        assert_eq!(auto_parallel_files(0, 0, 0), MIN_AUTO_FILES);
        assert_eq!(auto_parallel_files(1, 10_000, 20), MIN_AUTO_FILES);
    }

    #[test]
    fn large_files_stay_at_two() {
        // Ten connections pipelined twice: a window of 20. Two files of 68 articles (a 50 MB
        // RAR volume at 750 KB an article) hold far more than two windows.
        assert_eq!(auto_parallel_files(2, 136, 20), 2);
    }

    #[test]
    fn another_file_starts_while_the_open_articles_do_not_fill_two_windows() {
        assert_eq!(auto_parallel_files(2, 39, 20), 3);
        assert_eq!(auto_parallel_files(2, 40, 20), 2);
        assert_eq!(auto_parallel_files(3, 12, 20), 4);
    }

    #[test]
    fn small_files_settle_near_two_windows_over_their_size() {
        // Files of five articles against a window of 20: the count grows one file per pass
        // until the open articles reach 40 or the cap stops it.
        let mut running = MIN_AUTO_FILES;
        loop {
            let next = auto_parallel_files(running, running * 5, 20);
            if next == running {
                break;
            }
            running = next;
        }
        assert_eq!(
            running, MAX_PARALLEL_FILES,
            "40 / 5 = 8, which is also the cap"
        );
        let mut running = MIN_AUTO_FILES;
        loop {
            let next = auto_parallel_files(running, running * 15, 20);
            if next == running {
                break;
            }
            running = next;
        }
        assert_eq!(running, 3, "ceil(40 / 15) = 3");
    }

    #[test]
    fn the_cap_holds_however_starved_the_window_is() {
        assert_eq!(
            auto_parallel_files(MAX_PARALLEL_FILES, 0, 64),
            MAX_PARALLEL_FILES
        );
        assert_eq!(auto_parallel_files(40, 0, 64), MAX_PARALLEL_FILES);
    }

    #[test]
    fn without_a_pool_nothing_grows_past_the_floor() {
        assert_eq!(auto_parallel_files(2, 0, 0), 2);
    }

    #[test]
    fn a_fixed_number_wins_and_is_capped() {
        let load = Arc::new(FileLoad::default());
        load.set_window(20);
        assert_eq!(load.capacity(1), 1);
        assert_eq!(load.capacity(4), 4);
        assert_eq!(load.capacity(50), MAX_PARALLEL_FILES);
    }

    #[test]
    fn the_load_follows_the_files_and_their_answers() {
        let load = Arc::new(FileLoad::default());
        load.set_window(20);
        let first = load.start(10);
        let second = load.start(10);
        assert_eq!(load.running(), 2);
        assert_eq!(
            load.capacity(0),
            3,
            "20 open articles do not fill two windows of 20"
        );
        let third = load.start(30);
        assert_eq!(load.capacity(0), 3, "50 open articles do");
        for _ in 0..25 {
            third.answered();
        }
        assert_eq!(load.capacity(0), 4, "25 left");
        // More answers than the file announced (a resume that re-queued a segment) never
        // take the count below what the file itself contributed.
        for _ in 0..20 {
            first.answered();
        }
        drop(third);
        assert_eq!(load.running(), 2);
        assert_eq!(
            load.open_articles
                .load(std::sync::atomic::Ordering::Acquire),
            10,
            "only the second file's ten are left"
        );
        drop(first);
        drop(second);
        assert_eq!(load.running(), 0);
        assert_eq!(
            load.open_articles
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }

    #[test]
    fn an_untracked_file_counts_nothing() {
        let open = super::OpenArticles::untracked();
        open.answered();
        open.wrote(3, std::time::Duration::from_millis(1));
    }
}

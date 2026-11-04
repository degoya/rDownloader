//! How one scrape of a torrent's trackers runs: several trackers at once, under one deadline,
//! at most one run per torrent, and the counters folded into the stored state at the end.

use std::{collections::HashSet, future::Future, sync::Mutex, time::Duration};

use futures_util::{StreamExt, stream};
use rd_core::{DownloadId, TorrentJobState, TrackerScrape};

/// Trackers asked at the same time during one scrape.
pub(crate) const SCRAPE_PARALLEL: usize = 8;

/// Longest one scrape of a torrent runs, however many trackers it has.
pub(crate) const SCRAPE_DEADLINE: Duration = Duration::from_secs(30);

/// What a tracker that had not answered by the deadline records.
const NO_ANSWER_IN_TIME: &str = "the tracker did not answer within the scrape deadline";

/// The answer of one tracker, by its URL.
pub(crate) type ScrapeResult = (String, anyhow::Result<TrackerScrape>);

/// Asks every tracker through `scrape_one`, `parallel` at a time, and returns the answers
/// that arrived within `deadline`; a tracker still pending then is left out.
pub(crate) async fn scrape_all<F, Fut>(
    urls: Vec<String>,
    scrape_one: F,
    parallel: usize,
    deadline: Duration,
) -> Vec<ScrapeResult>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ScrapeResult>,
{
    stream::iter(urls)
        .map(scrape_one)
        .buffer_unordered(parallel.max(1))
        .take_until(tokio::time::sleep(deadline))
        .collect()
        .await
}

/// Folds scrape answers into the stored state.
///
/// Only trackers whose URL was asked (`asked`) are touched: an answer sets the counters, a
/// failure or a missing answer sets the reason and keeps the previous counters. A tracker
/// added since keeps whatever it has.
pub(crate) fn apply_scrapes(
    state: &mut TorrentJobState,
    asked: &[String],
    results: Vec<ScrapeResult>,
) {
    let Some(metadata) = state.metadata.as_mut() else {
        return;
    };
    // The URL may hold a passkey, so only the reason of a failure is kept.
    let answers: Vec<(String, Result<TrackerScrape, String>)> = results
        .into_iter()
        .map(|(url, result)| (url, result.map_err(|error| format!("{error:#}"))))
        .collect();
    for tracker in &mut metadata.trackers {
        if !asked.contains(&tracker.url) {
            continue;
        }
        match answers.iter().find(|(url, _)| *url == tracker.url) {
            Some((_, Ok(counters))) => {
                tracker.scrape = Some(*counters);
                tracker.last_error = None;
            }
            Some((_, Err(reason))) => tracker.last_error = Some(reason.clone()),
            None => tracker.last_error = Some(NO_ANSWER_IN_TIME.to_owned()),
        }
    }
}

/// The mark that one torrent is being scraped; dropping it clears the mark.
pub(crate) struct ScrapeClaim<'a> {
    running: &'a Mutex<HashSet<DownloadId>>,
    id: DownloadId,
}

impl<'a> ScrapeClaim<'a> {
    /// Marks `id` as being scraped, or `None` when a scrape of it already runs.
    pub(crate) fn take(running: &'a Mutex<HashSet<DownloadId>>, id: DownloadId) -> Option<Self> {
        let inserted = running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id);
        inserted.then_some(Self { running, id })
    }
}

impl Drop for ScrapeClaim<'_> {
    fn drop(&mut self) {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

#[cfg(test)]
#[path = "scrape_run_tests.rs"]
mod tests;

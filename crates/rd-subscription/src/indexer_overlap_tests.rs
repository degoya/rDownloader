//! Paging until the poll meets what the subscription already has (RD-1150-05).
//!
//! The fakes number entries by their position at the time of the poll, newest first, and the
//! archive holds every entry from some number on: what earlier polls stored. The interface warns
//! about a gap when a run found more than a first poll can and every entry was new; these tests
//! hold the two halves of that — where paging stops, and whether an archived entry came along.

use super::{Archive, PagingFetcher, StaticKey, archive, indexer_subscription, paging};
use crate::adapter::{RateLimited, SourceAdapter};
use crate::indexer::{IndexerAdapter, MAX_INDEXER_ITEMS, MAX_PAGES};
use std::sync::Arc;

/// Polls a primed subscription whose archive holds the entries from `known_from` on.
async fn poll(
    fetcher: &Arc<PagingFetcher>,
    known_from: Option<usize>,
) -> (anyhow::Result<crate::adapter::PollOutcome>, Arc<Archive>) {
    let archive = archive(known_from);
    let outcome = IndexerAdapter::new(fetcher.clone(), Arc::new(StaticKey), archive.clone())
        .poll(&indexer_subscription())
        .await;
    (outcome, archive)
}

fn requests(fetcher: &PagingFetcher) -> usize {
    fetcher.requested.lock().expect("lock").len()
}

fn has(outcome: &crate::adapter::PollOutcome, number: usize) -> bool {
    let wanted = format!("id-{number}");
    outcome
        .items
        .iter()
        .any(|item| item.source_id.as_deref() == Some(wanted.as_str()))
}

#[tokio::test]
async fn little_that_is_new_ends_the_poll_after_the_first_page() {
    let fetcher = Arc::new(paging(vec![100; 25]));
    let (outcome, archive) = poll(&fetcher, Some(5)).await;
    let outcome = outcome.expect("poll");

    assert_eq!(
        requests(&fetcher),
        1,
        "the first page already met the archive"
    );
    assert!((0..5).all(|number| has(&outcome, number)));
    // The archived entries of that page come along: the archive drops them again, and the run
    // that found more than it archived is no gap.
    assert_eq!(outcome.items.len(), 100);
    assert_eq!(
        archive.asked.lock().expect("lock").as_slice(),
        ["id:id-99"],
        "one lookup per page, for its last entry"
    );
}

#[tokio::test]
async fn seven_hundred_new_entries_take_eight_pages_and_all_arrive() {
    let fetcher = Arc::new(paging(vec![100; 25]));
    let (outcome, _) = poll(&fetcher, Some(700)).await;
    let outcome = outcome.expect("poll");

    assert_eq!(requests(&fetcher), 8);
    assert!(
        (0..700).all(|number| has(&outcome, number)),
        "every new entry reached the poll"
    );
    // The eighth page is the archived one, so the run found more than it archives: no warning.
    assert_eq!(outcome.items.len(), 800);
    assert!(has(&outcome, 799));
}

#[tokio::test]
async fn more_than_the_bound_reads_every_page_and_meets_nothing_archived() {
    let fetcher = Arc::new(paging(vec![100; 25]));
    let (outcome, archive) = poll(&fetcher, Some(2_400)).await;
    let outcome = outcome.expect("poll");

    assert_eq!(
        requests(&fetcher),
        usize::try_from(MAX_PAGES).expect("pages")
    );
    // Every page full, every entry new: the shape the interface warns about.
    assert_eq!(outcome.items.len(), MAX_INDEXER_ITEMS);
    assert!(!has(&outcome, MAX_INDEXER_ITEMS));
    assert_eq!(
        archive.asked.lock().expect("lock").len(),
        usize::try_from(MAX_PAGES).expect("pages")
    );
}

#[tokio::test]
async fn a_rate_limit_in_the_middle_ends_the_poll_as_a_pause() {
    let fetcher = Arc::new(PagingFetcher {
        rate_limit_offset: Some(200),
        ..paging(vec![100; 25])
    });
    let (outcome, _) = poll(&fetcher, Some(700)).await;
    let error = outcome.expect_err("the third page was refused");

    assert_eq!(
        error.downcast_ref::<RateLimited>(),
        Some(&RateLimited {
            until: super::rate_limit_until()
        }),
        "the server's pause reaches the poller unchanged: {error}"
    );
    assert_eq!(requests(&fetcher), 3, "nothing is asked after the refusal");
}

#[tokio::test]
async fn an_exhausted_request_allowance_is_a_pause_not_a_failure() {
    let fetcher = Arc::new(PagingFetcher {
        request_limit_offset: Some(100),
        ..paging(vec![100; 25])
    });
    let before = chrono::Utc::now();
    let (outcome, _) = poll(&fetcher, Some(700)).await;
    let error = outcome.expect_err("the second page was refused");

    let limited = error
        .downcast_ref::<RateLimited>()
        .unwrap_or_else(|| panic!("a request limit is a pause, not a failure: {error}"));
    assert!(limited.until >= before + chrono::Duration::minutes(59));
    assert_eq!(requests(&fetcher), 2);
}

#[tokio::test]
async fn an_indexer_that_ignores_the_offset_is_asked_twice_not_twenty_times() {
    let fetcher = Arc::new(PagingFetcher {
        ignores_offset: true,
        ..paging(vec![100; 25])
    });
    let (outcome, _) = poll(&fetcher, Some(2_400)).await;
    let outcome = outcome.expect("poll");

    assert_eq!(requests(&fetcher), 2);
    assert_eq!(outcome.items.len(), 100);
}

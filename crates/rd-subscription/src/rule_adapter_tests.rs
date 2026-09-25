//! A watched release page, against recorded answers only (RD-110-21).
//!
//! The two listings below are the same board page a week apart: the second carries one new
//! release, the same episode again from another group, and a page of navigation neither of
//! them should contribute. Nothing here touches the network.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use url::Url;

use rd_core::{
    BacklogPolicy, DownloadPriority, Subscription, SubscriptionFilters, SubscriptionId,
    SubscriptionKind, SubscriptionMode,
};

use super::{ClaimedAddresses, RuleAdapter};
use crate::{FeedFetcher, FetchedFeed, SourceAdapter, candidate_of, evaluate, key_of};

/// The first answer: two releases, plus the furniture every board page carries.
const FIRST: &str = r#"<html><body>
<nav><a href="/">Home</a><a href="/tv/">TV</a><a href="https://other.test/ad">Sponsor</a></nav>
<div class="post">
  <a class="thumb" href="/tv/the-expanse-s05e03-german-dl-1080p-web-x264-group/"><img src="a.jpg"></a>
  <h2><a href="/tv/the-expanse-s05e03-german-dl-1080p-web-x264-group/">The.Expanse.S05E03.German.DL.1080p.WEB.x264-GROUP</a></h2>
</div>
<div class="post">
  <h2><a href="/tv/the-expanse-s05e02-german-dl-720p-web-x264-group/">The.Expanse.S05E02.German.DL.720p.WEB.x264-GROUP</a></h2>
</div>
</body></html>"#;

/// A week later: S05E04 is new, and S05E03 is back as somebody else's release.
const SECOND: &str = r#"<html><body>
<nav><a href="/">Home</a><a href="/tv/">TV</a></nav>
<div class="post">
  <h2><a href="/tv/the-expanse-s05e04-english-2160p-web-x265-other/">The.Expanse.S05E04.English.2160p.WEB.x265-OTHER</a></h2>
</div>
<div class="post">
  <h2><a href="/tv/the-expanse-s05e03-english-720p-hdtv-x264-other/">The.Expanse.S05E03.English.720p.HDTV.x264-OTHER</a></h2>
</div>
<div class="post">
  <h2><a href="/tv/the-expanse-s05e03-german-dl-1080p-web-x264-group/">The.Expanse.S05E03.German.DL.1080p.WEB.x264-GROUP</a></h2>
</div>
</body></html>"#;

const LISTING: &str = "https://board.test/tv/the-expanse/";

/// Hands out the recorded answers in order, one per poll.
struct Recorded {
    pages: Mutex<Vec<&'static str>>,
}

impl Recorded {
    fn new(pages: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            pages: Mutex::new(pages.iter().rev().copied().collect()),
        })
    }
}

#[async_trait]
impl FeedFetcher for Recorded {
    async fn fetch(
        &self,
        _url: &Url,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> anyhow::Result<FetchedFeed> {
        let body = self
            .pages
            .lock()
            .expect("no other thread holds this")
            .pop()
            .map(str::to_owned);
        Ok(FetchedFeed {
            body,
            etag: None,
            last_modified: None,
            final_url: Some(LISTING.parse().expect("url")),
        })
    }
}

/// The rule in force: it claims release pages of this board and nothing else.
struct BoardRule;

impl ClaimedAddresses for BoardRule {
    fn claims(&self, url: &Url) -> bool {
        url.host_str() == Some("board.test")
            && url.path().starts_with("/tv/")
            && url.path().trim_end_matches('/').matches('/').count() == 2
    }
}

fn subscription(every_release: bool, filters: SubscriptionFilters) -> Subscription {
    Subscription {
        id: SubscriptionId::new(),
        name: "The Expanse".to_owned(),
        url: LISTING.parse().expect("url"),
        kind: SubscriptionKind::SiteRule,
        enabled: true,
        mode: SubscriptionMode::AutoQueue,
        category_id: None,
        priority: DownloadPriority::default(),
        interval_seconds: 3_600,
        filters,
        backlog: BacklogPolicy::ReviewAll,
        category_map: Vec::new(),
        source_categories: Vec::new(),
        primed: true,
        last_run_at: None,
        next_run_at: None,
        consecutive_failures: 0,
        last_error: None,
        etag: None,
        last_modified: None,
        secret_ref: None,
        has_secret: false,
        every_release,
        view: rd_core::SubscriptionView::List,
        autoplay: false,
        card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
        schedule: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn adapter(pages: &[&'static str]) -> RuleAdapter {
    RuleAdapter::new(Recorded::new(pages), Arc::new(BoardRule))
}

#[tokio::test]
async fn a_watched_page_delivers_its_releases_and_nothing_else() {
    let adapter = adapter(&[FIRST]);
    let outcome = adapter
        .poll(&subscription(false, SubscriptionFilters::default()))
        .await
        .expect("polled");
    let titles: Vec<&str> = outcome
        .items
        .iter()
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "The.Expanse.S05E03.German.DL.1080p.WEB.x264-GROUP",
            "The.Expanse.S05E02.German.DL.720p.WEB.x264-GROUP",
        ]
    );
    // The navigation, the board's own index and the sponsor are no release pages, and the
    // thumbnail link to the first release is the same address twice.
    assert_eq!(outcome.items.len(), 2);
    assert_eq!(
        outcome.items[0].url.as_str(),
        "https://board.test/tv/the-expanse-s05e03-german-dl-1080p-web-x264-group/"
    );
    // What the name said, so the ordinary filters judge a release without a second
    // mechanism.
    assert_eq!(outcome.items[0].height, Some(1080));
    assert_eq!(outcome.items[0].language.as_deref(), Some("German"));
    assert!(!outcome.not_modified);
}

#[tokio::test]
async fn a_second_poll_delivers_the_new_episode_and_not_the_one_already_had() {
    // The acceptance criterion, in the shape it is written: two consecutive answers, and
    // the archive that lies between them is the set of keys the first poll produced.
    let adapter = adapter(&[FIRST, SECOND]);
    let subscription = subscription(false, SubscriptionFilters::default());

    let first = adapter.poll(&subscription).await.expect("first poll");
    let archive: Vec<String> = first.items.iter().map(key_of).collect();
    assert_eq!(
        archive,
        [
            "release:the expanse|s05e03".to_owned(),
            "release:the expanse|s05e02".to_owned(),
        ]
    );

    let second = adapter.poll(&subscription).await.expect("second poll");
    assert_eq!(second.items.len(), 3, "the page still lists all three");
    let fresh: Vec<&str> = second
        .items
        .iter()
        .filter(|item| !archive.contains(&key_of(item)))
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        fresh,
        ["The.Expanse.S05E04.English.2160p.WEB.x265-OTHER"],
        "S05E03 by another group is the episode already had"
    );
    // Both postings of S05E03 carry one key although they are two addresses; that is the
    // whole recognition, and the UNIQUE index on (subscription, key) is what enforces it.
    assert_eq!(key_of(&second.items[1]), key_of(&second.items[2]));
    assert_ne!(second.items[1].url, second.items[2].url);
}

#[tokio::test]
async fn wanting_every_version_brings_the_second_release_back() {
    // The explicit counter-choice: the name then decides nothing and the address is the
    // identity, exactly as it is for every other kind of subscription.
    let adapter = adapter(&[FIRST, SECOND]);
    let subscription = subscription(true, SubscriptionFilters::default());

    let first = adapter.poll(&subscription).await.expect("first poll");
    let archive: Vec<String> = first.items.iter().map(key_of).collect();
    assert!(
        archive.iter().all(|key| key.starts_with("url:")),
        "{archive:?}"
    );

    let second = adapter.poll(&subscription).await.expect("second poll");
    let fresh: Vec<&str> = second
        .items
        .iter()
        .filter(|item| !archive.contains(&key_of(item)))
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        fresh,
        [
            "The.Expanse.S05E04.English.2160p.WEB.x265-OTHER",
            "The.Expanse.S05E03.English.720p.HDTV.x264-OTHER",
        ]
    );
}

#[tokio::test]
async fn quality_language_and_exclusion_filters_reach_a_release_name() {
    let adapter = adapter(&[SECOND]);
    let filters = SubscriptionFilters {
        min_height: Some(1080),
        languages: vec!["English".to_owned()],
        title_excludes: vec!["x265".to_owned()],
        ..SubscriptionFilters::default()
    };
    let subscription = subscription(false, filters);
    let outcome = adapter.poll(&subscription).await.expect("polled");
    let verdicts: Vec<(String, Option<String>)> = outcome
        .items
        .iter()
        .map(|item| {
            let decision = evaluate(
                &candidate_of(item),
                &subscription.filters,
                subscription.backlog,
                subscription.primed,
                chrono::Utc::now(),
            );
            (
                item.title.clone(),
                decision.err().map(|reason| reason.key().to_owned()),
            )
        })
        .collect();
    assert_eq!(
        verdicts,
        vec![
            // 2160p and English, but the exclusion wins over everything.
            (
                "The.Expanse.S05E04.English.2160p.WEB.x265-OTHER".to_owned(),
                Some("subscription.filter.title_excluded".to_owned())
            ),
            (
                "The.Expanse.S05E03.English.720p.HDTV.x264-OTHER".to_owned(),
                Some("subscription.filter.resolution_too_low".to_owned())
            ),
            (
                "The.Expanse.S05E03.German.DL.1080p.WEB.x264-GROUP".to_owned(),
                Some("subscription.filter.language_not_wanted".to_owned())
            ),
        ]
    );
}

#[tokio::test]
async fn an_unchanged_page_costs_nothing_and_deletes_nothing() {
    // A `304` is an empty list that means it. The archive only ever adds, so this changes
    // nothing either way -- but `not_modified` is what keeps the caller from reading it as
    // "the board removed everything".
    let adapter = adapter(&[]);
    let outcome = adapter
        .poll(&subscription(false, SubscriptionFilters::default()))
        .await
        .expect("polled");
    assert!(outcome.items.is_empty());
    assert!(outcome.not_modified);
}

#[tokio::test]
async fn the_adapter_serves_its_own_kind_only() {
    assert_eq!(adapter(&[]).kind(), SubscriptionKind::SiteRule);
}

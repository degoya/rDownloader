use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use rd_core::{
    DownloadId, TorrentJobState, TorrentMetadataInfo, TorrentTracker, TrackerOrigin, TrackerScrape,
};

use super::{ScrapeClaim, apply_scrapes, scrape_all};

fn counters(seeders: u32) -> TrackerScrape {
    TrackerScrape {
        seeders,
        leechers: 0,
        completed: 0,
        scraped_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn trackers_are_asked_in_parallel_up_to_the_limit() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let urls: Vec<String> = (0..20)
        .map(|n| format!("udp://tracker{n}.test:1"))
        .collect();
    let started = Instant::now();
    let results = scrape_all(
        urls,
        |url| {
            let in_flight = Arc::clone(&in_flight);
            let peak = Arc::clone(&peak);
            async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                (url, Ok(counters(1)))
            }
        },
        4,
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(results.len(), 20);
    assert_eq!(peak.load(Ordering::SeqCst), 4);
    // One after the other would take a second; four at a time about a quarter of that.
    assert!(started.elapsed() < Duration::from_millis(900));
}

#[tokio::test]
async fn the_deadline_ends_the_run_and_keeps_what_answered() {
    let started = Instant::now();
    let results = scrape_all(
        vec![
            "udp://fast.test:1".to_owned(),
            "udp://silent.test:1".to_owned(),
        ],
        |url| async move {
            if url.contains("silent") {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            (url, Ok(counters(2)))
        },
        8,
        Duration::from_millis(200),
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "udp://fast.test:1");
}

fn state_with(urls: &[&str]) -> TorrentJobState {
    TorrentJobState {
        metadata: Some(TorrentMetadataInfo {
            info_hash: "hash".to_owned(),
            name: "release".to_owned(),
            total_bytes: rd_core::ByteCount::new(1).expect("valid"),
            piece_length: 16_384,
            piece_count: 1,
            private: false,
            files: Vec::new(),
            trackers: urls
                .iter()
                .map(|url| TorrentTracker::new((*url).to_owned(), 0, TrackerOrigin::Metadata))
                .collect(),
            web_seeds: Vec::new(),
        }),
        ..TorrentJobState::default()
    }
}

#[test]
fn answers_land_only_on_the_trackers_that_were_asked() {
    let mut state = state_with(&[
        "udp://a.test:1",
        "udp://b.test:1",
        "udp://c.test:1",
        "udp://new.test:1",
    ]);
    let asked = vec![
        "udp://a.test:1".to_owned(),
        "udp://b.test:1".to_owned(),
        "udp://c.test:1".to_owned(),
    ];
    let results = vec![
        ("udp://a.test:1".to_owned(), Ok(counters(5))),
        ("udp://b.test:1".to_owned(), Err(anyhow::anyhow!("refused"))),
    ];
    apply_scrapes(&mut state, &asked, results);
    let trackers = &state.metadata.as_ref().expect("metadata").trackers;
    assert_eq!(trackers[0].scrape.map(|scrape| scrape.seeders), Some(5));
    assert_eq!(trackers[0].last_error, None);
    assert_eq!(trackers[1].last_error.as_deref(), Some("refused"));
    assert!(
        trackers[2]
            .last_error
            .as_deref()
            .is_some_and(|reason| reason.contains("deadline"))
    );
    // Added while the trackers were asked: untouched.
    assert_eq!(trackers[3].last_error, None);
    assert_eq!(trackers[3].scrape, None);
}

#[test]
fn one_scrape_per_torrent_at_a_time() {
    let running = Mutex::new(HashSet::new());
    let id = DownloadId::new();
    let first = ScrapeClaim::take(&running, id).expect("first");
    assert!(ScrapeClaim::take(&running, id).is_none());
    assert!(ScrapeClaim::take(&running, DownloadId::new()).is_some());
    drop(first);
    assert!(ScrapeClaim::take(&running, id).is_some());
}

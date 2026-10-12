//! A large request beside small ones at one bucket (RD-1270-25).
//!
//! A Usenet article (~700 KB) used to ask for its bytes in one batch as large as the bucket and
//! wait for the bucket to be full, while HTTP took 16 KiB per network read and never let it
//! fill: the article got nothing at all, however long it waited. Real time, since governor
//! keeps its own clock; the limits are high enough that each case takes about a second.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use super::BandwidthLimiter;

const RATE: u64 = 2 * 1024 * 1024;
const SMALL: usize = 16 * 1024;
const ARTICLE: usize = 700 * 1024;

/// A limiter whose bucket has just been emptied, so nothing rides on its initial burst.
async fn drained() -> BandwidthLimiter {
    let limiter = BandwidthLimiter::new(Some(RATE));
    limiter
        .acquire(RATE as usize)
        .await
        .expect("the full bucket");
    limiter
}

/// `count` tasks drawing small pieces back to back until `stop`, as HTTP downloads do.
fn small_drawers(
    limiter: &BandwidthLimiter,
    count: usize,
    stop: &Arc<AtomicBool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    (0..count)
        .map(|_| {
            let limiter = limiter.clone();
            let stop = Arc::clone(stop);
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    limiter.acquire(SMALL).await.expect("a small piece");
                }
            })
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_request_is_served_beside_small_ones_that_draw_without_pause() {
    let limiter = drained().await;
    let stop = Arc::new(AtomicBool::new(false));
    let drawers = small_drawers(&limiter, 4, &stop);
    // Let the small ones settle into drawing the bucket empty first.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    // At its turn among the four it gets about half the rate: 0.7 s. The old batch never came.
    tokio::time::timeout(Duration::from_secs(5), limiter.acquire(ARTICLE))
        .await
        .expect("the article was served in its turns, not starved")
        .expect("acquire");
    let waited = started.elapsed();

    stop.store(true, Ordering::Relaxed);
    for drawer in drawers {
        drawer.await.expect("drawer");
    }
    assert!(
        waited >= Duration::from_millis(250),
        "an empty bucket cannot serve {ARTICLE} bytes in {waited:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rate_over_mixed_requests_stays_at_the_limit() {
    let limiter = drained().await;
    let served = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let limiter = limiter.clone();
        let served = Arc::clone(&served);
        tasks.push(tokio::spawn(async move {
            for _ in 0..32 {
                limiter.acquire(SMALL).await.expect("a small piece");
                served.fetch_add(SMALL as u64, Ordering::Relaxed);
            }
        }));
    }
    for _ in 0..2 {
        let limiter = limiter.clone();
        let served = Arc::clone(&served);
        tasks.push(tokio::spawn(async move {
            limiter.acquire(ARTICLE).await.expect("an article");
            served.fetch_add(ARTICLE as u64, Ordering::Relaxed);
        }));
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("every request was served")
            .expect("task");
    }
    let elapsed = started.elapsed().as_secs_f64();
    let served = served.load(Ordering::Relaxed);
    // 3.4 MiB at 2 MiB/s: 1.7 s. Never faster than the limit — the bucket was empty — and
    // not much slower, which a piece lost between the turns would make it.
    let at_the_limit = served as f64 / RATE as f64;
    assert!(
        elapsed >= at_the_limit * 0.95,
        "{served} bytes in {elapsed:.2} s is faster than {RATE} B/s"
    );
    assert!(
        elapsed <= at_the_limit * 1.25 + 0.3,
        "{served} bytes in {elapsed:.2} s fell short of {RATE} B/s"
    );
}

#[tokio::test]
async fn a_limit_lifted_meanwhile_releases_the_rest_of_a_request() {
    let limiter = BandwidthLimiter::new(Some(64 * 1024));
    let waiting = tokio::spawn({
        let limiter = limiter.clone();
        // 160 s at the limit it starts under.
        async move { limiter.acquire(10 * 1024 * 1024).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    limiter.set_limit(None);
    tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .expect("the rest went through unlimited")
        .expect("task")
        .expect("acquire");
}

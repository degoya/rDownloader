//! One request at a time per indexer, with a gap between them (RD-101-18).
//!
//! Subscriptions are spread over time by their own id, so two created in the same minute do
//! not poll in the same second forever. That spread is per *subscription*, and it says
//! nothing about the server on the other end. Four subscriptions against the same indexer --
//! one per category, which is how people actually configure them -- become due together and,
//! under the parallelism budget, fire together. The recorded poll history of such a setup
//! shows several pairs of requests 0.0 seconds apart.
//!
//! That is a poor way to treat a server that counts requests, and worse for one that counts
//! *failed* requests: an indexer answering "your previous 2 attempts failed, try again in 300
//! seconds" is describing a lockout that a burst walks straight into.
//!
//! So polls to one host are serialized and spaced. The gate is on `poll_one`, which is the
//! single road both the scheduled cycle and the manual "check now" take -- a manual trigger
//! landing on top of a due cycle is exactly the case that produced the tightest pairs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Quiet time between two requests to the same host.
///
/// Not a rate limit -- an indexer's own limit is counted per day and is nowhere near this.
/// It is the difference between arriving as several clients at once and arriving as one, and
/// two seconds is enough to be the latter without making a cycle of a handful of
/// subscriptions feel slow.
pub(crate) const MIN_HOST_INTERVAL: Duration = Duration::from_secs(2);

/// Above this many remembered hosts, entries nobody is using are dropped.
///
/// The key set is bounded by the user's subscriptions in practice; the cap only stops a long
/// run of edits from growing the map without end.
const MAX_TRACKED_HOSTS: usize = 256;

/// Serializes and spaces requests per host.
pub(crate) struct HostGate {
    minimum: Duration,
    /// Held only long enough to clone an `Arc`; never across an await.
    hosts: Mutex<HashMap<String, Arc<AsyncMutex<Option<Instant>>>>>,
}

/// The right to talk to one host. Records when it is given up, so the gap is measured from
/// the end of one request to the start of the next rather than between two starts -- which
/// is what a server actually experiences.
pub(crate) struct HostPermit {
    guard: OwnedMutexGuard<Option<Instant>>,
}

impl Drop for HostPermit {
    fn drop(&mut self) {
        *self.guard = Some(Instant::now());
    }
}

impl HostGate {
    pub(crate) fn new(minimum: Duration) -> Self {
        Self {
            minimum,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Waits until it is this host's turn and the quiet time has passed.
    ///
    /// `None` for an address with no host -- there is nothing to serialize against, and
    /// refusing to poll over it would be a worse answer than letting it through.
    pub(crate) async fn enter(&self, url: &url::Url) -> Option<HostPermit> {
        let host = url.host_str()?.to_ascii_lowercase();
        let slot = self.slot(host)?;
        let guard = slot.lock_owned().await;
        if let Some(last) = *guard {
            let elapsed = last.elapsed();
            if elapsed < self.minimum {
                tokio::time::sleep(self.minimum - elapsed).await;
            }
        }
        Some(HostPermit { guard })
    }

    fn slot(&self, host: String) -> Option<Arc<AsyncMutex<Option<Instant>>>> {
        // A poisoned lock would mean a panic while holding it, and there is nothing but a
        // map behind it; polling on without spacing beats failing the poll.
        let mut hosts = self.hosts.lock().ok()?;
        if hosts.len() >= MAX_TRACKED_HOSTS {
            // Anything nobody is waiting on or holding is only a remembered timestamp.
            hosts.retain(|_, slot| Arc::strong_count(slot) > 1);
        }
        Some(Arc::clone(hosts.entry(host).or_default()))
    }
}

impl Default for HostGate {
    fn default() -> Self {
        Self::new(MIN_HOST_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::HostGate;
    use std::time::{Duration, Instant};

    fn url(input: &str) -> url::Url {
        input.parse().expect("url")
    }

    #[tokio::test]
    async fn two_polls_of_one_host_do_not_overlap() {
        // The reported shape: several subscriptions against one indexer, due together.
        let gate = std::sync::Arc::new(HostGate::new(Duration::from_millis(0)));
        let held = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let gate = std::sync::Arc::clone(&gate);
            let held = std::sync::Arc::clone(&held);
            let peak = std::sync::Arc::clone(&peak);
            tasks.push(tokio::spawn(async move {
                let permit = gate.enter(&url("https://indexer.test/api?cat=2040")).await;
                let now = held.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                held.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                drop(permit);
            }));
        }
        for task in tasks {
            task.await.expect("task");
        }
        assert_eq!(
            peak.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "requests to one host must not run at the same time"
        );
    }

    #[tokio::test]
    async fn a_second_request_waits_out_the_quiet_time() {
        let gate = HostGate::new(Duration::from_millis(120));
        let started = Instant::now();
        drop(gate.enter(&url("https://indexer.test/api")).await);
        drop(gate.enter(&url("https://indexer.test/api")).await);
        assert!(
            started.elapsed() >= Duration::from_millis(120),
            "the second request came after {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn different_hosts_do_not_wait_for_each_other() {
        // A slow indexer must not hold up a channel somewhere else.
        let gate = HostGate::new(Duration::from_secs(30));
        let started = Instant::now();
        drop(gate.enter(&url("https://one.test/api")).await);
        drop(gate.enter(&url("https://two.test/api")).await);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn the_same_host_under_a_different_address_is_still_the_same_host() {
        // Different category queries against one indexer are the case this exists for.
        let gate = HostGate::new(Duration::from_millis(120));
        let started = Instant::now();
        drop(gate.enter(&url("https://Indexer.test/api?cat=2040")).await);
        drop(gate.enter(&url("https://indexer.test/api?cat=5030")).await);
        assert!(started.elapsed() >= Duration::from_millis(120));
    }

    #[tokio::test]
    async fn an_address_without_a_host_is_let_through() {
        let gate = HostGate::new(Duration::from_secs(30));
        assert!(gate.enter(&url("file:///tmp/feed.xml")).await.is_none());
    }
}

//! Per-host connection policy: how many requests rDownloader keeps open against one host,
//! and which hosts have proven that they answer a range request with the whole file.
//!
//! Nothing used to bound this. A file is split into up to four chunks, each chunk is its
//! own concurrent ranged `GET`, and several files of the same hoster run at once, so one
//! CDN could see dozens of simultaneous connections from a single installation. Hosters
//! answer that with throttling, a landing page or a plain refusal -- which is exactly what
//! "it fails when I add several links at once" looks like from the outside.
//!
//! The limit is shared by every transfer, so it holds across files as well as across the
//! chunks of one file. Bytes per second are a different, orthogonal budget and stay in
//! `rd-limits`.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

/// Simultaneous connections one host may see, unless the operator says otherwise.
///
/// Six is the number every browser settled on for HTTP/1.1 after RFC 2616's two proved
/// unusable, and it is therefore the load hosters are built for. It also still allows the
/// default four chunks of one file to run in parallel.
pub const DEFAULT_CONNECTIONS_PER_HOST: usize = 6;

/// Upper bound accepted for the configurable value; `0` switches the limit off.
pub const MAX_CONNECTIONS_PER_HOST: usize = 32;

/// Hosts kept in the slot table before idle entries are swept out.
const MAX_TRACKED_HOSTS: usize = 256;

/// Shared, runtime-configurable connection policy per host.
#[derive(Clone)]
pub struct HostLimits {
    inner: Arc<Inner>,
}

struct Inner {
    /// Simultaneous connections per host; `0` means unlimited.
    limit: AtomicUsize,
    slots: Mutex<HashMap<String, Arc<Semaphore>>>,
    ignores_ranges: Mutex<HashSet<String>>,
}

impl Default for HostLimits {
    fn default() -> Self {
        Self::new(DEFAULT_CONNECTIONS_PER_HOST)
    }
}

impl std::fmt::Debug for HostLimits {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostLimits")
            .field("limit", &self.limit())
            .finish_non_exhaustive()
    }
}

impl HostLimits {
    /// Creates a policy allowing `limit` simultaneous connections per host.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                limit: AtomicUsize::new(limit.min(MAX_CONNECTIONS_PER_HOST)),
                slots: Mutex::new(HashMap::new()),
                ignores_ranges: Mutex::new(HashSet::new()),
            }),
        }
    }

    /// A policy that never makes a request wait; the shape tests and one-off callers want.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::new(0)
    }

    /// Simultaneous connections currently allowed per host.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.inner.limit.load(Ordering::Acquire)
    }

    /// Applies a changed setting without restarting anything.
    pub fn set_limit(&self, limit: usize) {
        let limit = limit.min(MAX_CONNECTIONS_PER_HOST);
        if self.inner.limit.swap(limit, Ordering::Release) == limit {
            return;
        }
        // A semaphore cannot be shrunk below the permits it has already handed out, so the
        // new limit takes effect through new semaphores: requests that are running keep
        // theirs alive until they finish, everything started afterwards sees the new value.
        if let Ok(mut slots) = self.inner.slots.lock() {
            slots.clear();
        }
    }

    /// Waits for a free slot on `url`'s host. The returned permit holds it until dropped.
    ///
    /// `None` means no slot was needed -- the limit is off, or the URL has no host.
    pub async fn acquire(&self, url: &Url) -> Option<OwnedSemaphorePermit> {
        let limit = self.limit();
        if limit == 0 {
            return None;
        }
        let host = host_key(url)?;
        let semaphore = {
            let mut slots = self.inner.slots.lock().ok()?;
            if slots.len() >= MAX_TRACKED_HOSTS {
                // Nobody holds a permit on these, so dropping them loses no state.
                slots.retain(|_, semaphore| Arc::strong_count(semaphore) > 1);
            }
            Arc::clone(
                slots
                    .entry(host)
                    .or_insert_with(|| Arc::new(Semaphore::new(limit))),
            )
        };
        // Entries are never closed, only dropped, so the error arm is unreachable.
        semaphore.acquire_owned().await.ok()
    }

    /// Records that this host answered a ranged request with something else.
    ///
    /// Remembered per process rather than persisted: it costs one failed attempt to learn
    /// again after a restart, and a host that was merely overloaded is not written off for
    /// good.
    pub fn note_ranges_ignored(&self, url: &Url) {
        let Some(host) = host_key(url) else {
            return;
        };
        if let Ok(mut hosts) = self.inner.ignores_ranges.lock() {
            hosts.insert(host);
        }
    }

    /// Whether a transfer from this host should be planned as a single connection.
    #[must_use]
    pub fn ignores_ranges(&self, url: &Url) -> bool {
        let Some(host) = host_key(url) else {
            return false;
        };
        self.inner
            .ignores_ranges
            .lock()
            .is_ok_and(|hosts| hosts.contains(&host))
    }
}

/// Host without port: the connection budget belongs to the server, not to the scheme.
fn host_key(url: &Url) -> Option<String> {
    url.host_str().map(str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use url::Url;

    use super::{DEFAULT_CONNECTIONS_PER_HOST, HostLimits};

    fn url(text: &str) -> Url {
        text.parse().expect("fixture URL")
    }

    /// Whether a slot on `target` is refused for as long as a caller is willing to wait.
    async fn blocked(limits: &HostLimits, target: &Url) -> bool {
        tokio::time::timeout(Duration::from_millis(50), limits.acquire(target))
            .await
            .is_err()
    }

    #[tokio::test]
    async fn one_host_hands_out_no_more_permits_than_the_limit() {
        let limits = HostLimits::new(2);
        let target = url("https://cdn.example.test/file");
        let first = limits.acquire(&target).await;
        let second = limits.acquire(&target).await;
        assert!(first.is_some() && second.is_some());
        assert!(
            blocked(&limits, &target).await,
            "the third was not held back"
        );
        drop(first);
        assert!(
            !blocked(&limits, &target).await,
            "a released slot was not handed on"
        );
    }

    #[tokio::test]
    async fn another_host_has_its_own_budget() {
        let limits = HostLimits::new(1);
        let held = limits.acquire(&url("https://one.example.test/file")).await;
        assert!(held.is_some());
        assert!(
            !blocked(&limits, &url("https://two.example.test/file")).await,
            "a second host waited for the first"
        );
    }

    #[tokio::test]
    async fn switching_the_limit_off_stops_making_requests_wait() {
        let limits = HostLimits::new(1);
        let target = url("https://cdn.example.test/file");
        let held = limits.acquire(&target).await;
        assert!(held.is_some());
        limits.set_limit(0);
        assert_eq!(limits.limit(), 0);
        assert!(limits.acquire(&target).await.is_none());
    }

    #[test]
    fn a_host_that_ignored_ranges_is_remembered_by_host_not_by_link() {
        let limits = HostLimits::default();
        assert_eq!(limits.limit(), DEFAULT_CONNECTIONS_PER_HOST);
        let first = url("https://cdn.example.test/a?token=1");
        let second = url("https://cdn.example.test/b?token=2");
        let elsewhere = url("https://other.example.test/c");
        assert!(!limits.ignores_ranges(&first));
        limits.note_ranges_ignored(&first);
        assert!(limits.ignores_ranges(&second));
        assert!(!limits.ignores_ranges(&elsewhere));
    }
}

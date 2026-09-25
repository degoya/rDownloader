//! The reconciliation interval as a value every watcher follows (RD-110-31).

use std::{sync::Arc, time::Duration};

use tokio::sync::watch;

/// The reconciliation interval, shared by every watcher and changeable while they run.
///
/// The service that owns the watchers holds this and calls [`set`](Self::set) when the setting
/// changes (RD-110-31); each watcher subscribes once at start and re-arms its ticker on every
/// change, without a restart. Restarting would have thrown away the stability observations and
/// the digest set that keeps one file from being imported twice.
#[derive(Clone, Debug)]
pub struct PollInterval {
    sender: Arc<watch::Sender<Duration>>,
}

impl PollInterval {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            sender: Arc::new(watch::Sender::new(interval)),
        }
    }

    /// The interval in force.
    #[must_use]
    pub fn get(&self) -> Duration {
        *self.sender.borrow()
    }

    /// Changes the interval for every watcher; the same value again wakes nobody.
    pub fn set(&self, interval: Duration) {
        self.sender.send_if_modified(|current| {
            let changed = *current != interval;
            *current = interval;
            changed
        });
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Duration> {
        self.sender.subscribe()
    }
}

impl Default for PollInterval {
    fn default() -> Self {
        Self::new(super::DEFAULT_RECONCILIATION)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::PollInterval;

    #[test]
    fn the_same_interval_again_wakes_nobody() {
        let interval = PollInterval::new(Duration::from_secs(30));
        let mut receiver = interval.subscribe();
        interval.set(Duration::from_secs(30));
        assert!(!receiver.has_changed().expect("open"));
        interval.set(Duration::from_secs(45));
        assert!(receiver.has_changed().expect("open"));
        assert_eq!(*receiver.borrow_and_update(), Duration::from_secs(45));
        assert_eq!(interval.get(), Duration::from_secs(45));
    }

    #[test]
    fn the_default_is_the_thirty_seconds_every_installation_had() {
        assert_eq!(PollInterval::default().get(), Duration::from_secs(30));
    }
}

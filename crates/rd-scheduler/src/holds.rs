//! Reasons the queue is holding back new starts, keyed by who asked.
//!
//! There is more than one thing that wants to hold the queue and they do not know about each
//! other: the power supervisor re-asserts the battery and metered context every few seconds,
//! while a reconnect holds the queue for the length of one attempt. A single slot meant the
//! supervisor's next tick silently cleared whatever else had been set, so each source now owns
//! its own entry and the queue runs again only when every one of them has let go.

use std::collections::BTreeMap;

use tokio::sync::Mutex;

/// Who is holding the queue. A fixed set rather than free text, so a typo cannot leave a hold
/// nobody can clear.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HoldSource {
    /// Battery or metered connection.
    Power,
    /// A reconnect is in progress.
    Reconnect,
}

/// The holds currently in force.
#[derive(Debug, Default)]
pub struct Holds {
    reasons: Mutex<BTreeMap<HoldSource, &'static str>>,
}

impl Holds {
    /// Sets or clears one source's hold, leaving the others alone.
    pub async fn set(&self, source: HoldSource, reason: Option<&'static str>) {
        let mut reasons = self.reasons.lock().await;
        match reason {
            Some(reason) => reasons.insert(source, reason),
            None => reasons.remove(&source),
        };
    }

    /// The reason to report, if the queue is held at all.
    ///
    /// Ordered by source rather than by when it was set, so the same combination always reads
    /// the same way.
    pub async fn reason(&self) -> Option<&'static str> {
        self.reasons.lock().await.values().next().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::{HoldSource, Holds};

    #[tokio::test]
    async fn nothing_holds_an_untouched_queue() {
        assert_eq!(Holds::default().reason().await, None);
    }

    #[tokio::test]
    async fn one_source_letting_go_does_not_release_another() {
        let holds = Holds::default();
        holds.set(HoldSource::Power, Some("battery")).await;
        holds.set(HoldSource::Reconnect, Some("reconnect")).await;

        // This is the bug the type exists for: the power supervisor re-asserts its own state
        // every few seconds and used to clear the reconnect's hold on the way past.
        holds.set(HoldSource::Power, None).await;

        assert_eq!(holds.reason().await, Some("reconnect"));
    }

    #[tokio::test]
    async fn the_queue_runs_once_every_source_has_let_go() {
        let holds = Holds::default();
        holds.set(HoldSource::Power, Some("metered")).await;
        holds.set(HoldSource::Reconnect, Some("reconnect")).await;

        holds.set(HoldSource::Reconnect, None).await;
        assert_eq!(holds.reason().await, Some("metered"));

        holds.set(HoldSource::Power, None).await;
        assert_eq!(holds.reason().await, None);
    }

    #[tokio::test]
    async fn setting_the_same_source_twice_replaces_its_reason() {
        let holds = Holds::default();
        holds.set(HoldSource::Power, Some("battery")).await;
        holds.set(HoldSource::Power, Some("metered")).await;

        assert_eq!(holds.reason().await, Some("metered"));

        holds.set(HoldSource::Power, None).await;
        assert_eq!(holds.reason().await, None, "one entry, not two");
    }
}

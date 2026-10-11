//! Bursts of one event folded into one notification (RD-1240-17).
//!
//! A hundred links pasted one after another, or a package of fifty files starting at once, is
//! one moment to the person being told about it, not a hundred messages. A burst stays open
//! while its occurrences follow each other within the quiet period and closes after it, or at
//! the longest span at the latest, so a steady trickle still reports now and then. Pure
//! bookkeeping: the clock is the caller's, so the tests need no timer.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rd_core::CategoryId;

use crate::NotificationEvent;

/// Seconds without a further occurrence after which a burst is reported.
pub const QUIET_SECONDS: i64 = 10;

/// Seconds after its first occurrence at which a burst is reported however busy it still is.
pub const LONGEST_SECONDS: i64 = 60;

/// How many names a burst keeps for its text; the rest is only counted.
pub const NAMED: usize = 3;

/// One occurrence of a coalescing event, as the bus event that carried it describes it.
#[derive(Clone, Debug)]
pub struct Occurrence {
    pub event: NotificationEvent,
    pub category_id: Option<CategoryId>,
    /// The bus event's id.
    pub event_id: String,
    /// What it carried: links for `links_added`, downloads for `download_started`.
    pub items: u64,
    /// What to call it in the text: a file name, an intake source.
    pub name: Option<String>,
}

/// Every occurrence of one event in one category between its opening and its report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Burst {
    pub event: NotificationEvent,
    pub category_id: Option<CategoryId>,
    /// The bus event that opened the burst. The deliveries' idempotency keys derive from it, so
    /// a burst reported twice still reaches each rule once.
    pub first_event_id: String,
    /// How many bus events were folded in.
    pub occurrences: u64,
    /// What they carried together.
    pub items: u64,
    /// The first [`NAMED`] distinct names, in the order they arrived.
    pub names: Vec<String>,
    opened_at: DateTime<Utc>,
    last_at: DateTime<Utc>,
}

/// The open bursts, one per event and category: a rule bound to a category must still see
/// only its own.
#[derive(Debug)]
pub struct Coalescer {
    quiet: Duration,
    longest: Duration,
    open: HashMap<(NotificationEvent, Option<CategoryId>), Burst>,
}

impl Default for Coalescer {
    fn default() -> Self {
        Self::new(
            Duration::seconds(QUIET_SECONDS),
            Duration::seconds(LONGEST_SECONDS),
        )
    }
}

impl Coalescer {
    #[must_use]
    pub fn new(quiet: Duration, longest: Duration) -> Self {
        Self {
            quiet,
            longest,
            open: HashMap::new(),
        }
    }

    /// Folds one occurrence into its burst, opening one when none is open.
    pub fn push(&mut self, occurrence: Occurrence, now: DateTime<Utc>) {
        let burst = self
            .open
            .entry((occurrence.event, occurrence.category_id))
            .or_insert_with(|| Burst {
                event: occurrence.event,
                category_id: occurrence.category_id,
                first_event_id: occurrence.event_id.clone(),
                occurrences: 0,
                items: 0,
                names: Vec::new(),
                opened_at: now,
                last_at: now,
            });
        burst.occurrences = burst.occurrences.saturating_add(1);
        burst.items = burst.items.saturating_add(occurrence.items);
        burst.last_at = now;
        if let Some(name) = occurrence.name
            && burst.names.len() < NAMED
            && !burst.names.contains(&name)
        {
            burst.names.push(name);
        }
    }

    /// Closes and returns the bursts that are due at `now`, oldest first.
    pub fn due(&mut self, now: DateTime<Utc>) -> Vec<Burst> {
        let (quiet, longest) = (self.quiet, self.longest);
        let keys: Vec<_> = self
            .open
            .iter()
            .filter(|(_, burst)| now - burst.last_at >= quiet || now - burst.opened_at >= longest)
            .map(|(key, _)| *key)
            .collect();
        let mut due: Vec<Burst> = keys
            .iter()
            .filter_map(|key| self.open.remove(key))
            .collect();
        due.sort_by_key(|burst| burst.opened_at);
        due
    }

    /// Closes and returns every open burst, for a shutdown that should not lose them.
    pub fn drain(&mut self) -> Vec<Burst> {
        let mut all: Vec<Burst> = self.open.drain().map(|(_, burst)| burst).collect();
        all.sort_by_key(|burst| burst.opened_at);
        all
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use rd_core::CategoryId;

    use super::{Coalescer, NAMED, Occurrence};
    use crate::NotificationEvent;

    fn links(id: &str, items: u64, name: &str) -> Occurrence {
        Occurrence {
            event: NotificationEvent::LinksAdded,
            category_id: None,
            event_id: id.to_owned(),
            items,
            name: Some(name.to_owned()),
        }
    }

    #[test]
    fn a_hundred_imports_in_a_row_are_one_burst() {
        let mut coalescer = Coalescer::default();
        let start = Utc::now();
        for second in 0..100 {
            coalescer.push(
                links(&format!("evt-{second}"), 2, "clipboard"),
                start + Duration::milliseconds(second * 100),
            );
            assert!(
                coalescer
                    .due(start + Duration::milliseconds(second * 100))
                    .is_empty()
            );
        }
        let due = coalescer.due(start + Duration::seconds(30));
        assert_eq!(due.len(), 1, "{due:?}");
        assert_eq!(due[0].occurrences, 100);
        assert_eq!(due[0].items, 200);
        assert_eq!(due[0].first_event_id, "evt-0");
        assert_eq!(due[0].names, vec!["clipboard".to_owned()]);
        assert!(coalescer.is_empty());
    }

    #[test]
    fn a_burst_waits_for_the_quiet_period() {
        let mut coalescer = Coalescer::new(Duration::seconds(10), Duration::seconds(60));
        let start = Utc::now();
        coalescer.push(links("a", 1, "api"), start);
        assert!(coalescer.due(start + Duration::seconds(9)).is_empty());
        coalescer.push(links("b", 1, "api"), start + Duration::seconds(9));
        // The second occurrence moved the quiet period on.
        assert!(coalescer.due(start + Duration::seconds(15)).is_empty());
        assert_eq!(coalescer.due(start + Duration::seconds(19)).len(), 1);
    }

    #[test]
    fn a_steady_trickle_still_reports_at_the_longest_span() {
        let mut coalescer = Coalescer::new(Duration::seconds(10), Duration::seconds(60));
        let start = Utc::now();
        let mut reported = Vec::new();
        for second in (0..=130).step_by(5) {
            let now = start + Duration::seconds(second);
            coalescer.push(links(&format!("evt-{second}"), 1, "api"), now);
            reported.extend(coalescer.due(now));
        }
        // Two full minutes of one import every five seconds: two reports, not twenty-five.
        assert_eq!(reported.len(), 2, "{reported:?}");
        assert_eq!(reported[0].first_event_id, "evt-0");
        assert_eq!(reported[1].first_event_id, "evt-65");
        assert!(!coalescer.is_empty());
        assert_eq!(coalescer.drain().len(), 1);
    }

    #[test]
    fn categories_and_events_keep_bursts_of_their_own() {
        let mut coalescer = Coalescer::default();
        let start = Utc::now();
        let films = CategoryId::new();
        let started = |id: &str, category_id| Occurrence {
            event: NotificationEvent::DownloadStarted,
            category_id,
            event_id: id.to_owned(),
            items: 1,
            name: Some(format!("{id}.mkv")),
        };
        coalescer.push(started("one", Some(films)), start);
        coalescer.push(started("two", None), start);
        coalescer.push(links("three", 4, "api"), start);
        let due = coalescer.due(start + Duration::seconds(11));
        assert_eq!(due.len(), 3, "{due:?}");
        assert!(due.iter().any(|burst| burst.category_id == Some(films)
            && burst.event == NotificationEvent::DownloadStarted
            && burst.names == vec!["one.mkv".to_owned()]));
    }

    #[test]
    fn a_burst_names_only_its_first_few_and_counts_the_rest() {
        let mut coalescer = Coalescer::default();
        let start = Utc::now();
        for index in 0..10 {
            coalescer.push(
                links(&format!("evt-{index}"), 1, &format!("name-{index}")),
                start,
            );
        }
        coalescer.push(links("again", 1, "name-0"), start);
        let due = coalescer.drain();
        assert_eq!(due[0].names.len(), NAMED);
        assert_eq!(due[0].names[0], "name-0");
        assert_eq!(due[0].occurrences, 11);
    }
}

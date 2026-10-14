//! Bursts of one event folded into one notification (RD-1240-17).
//!
//! A hundred links pasted one after another, or a package of fifty files starting at once, is
//! one moment to the person being told about it, not a hundred messages. A burst stays open
//! while its occurrences follow each other within the quiet period and closes after it, or at
//! the longest span at the latest, so a steady trickle still reports now and then. Pure
//! bookkeeping: the clock is the caller's, so the tests need no timer.
//!
//! A closed burst is worded once (RD-1291-01): its built-in text, or a rule's own template filled
//! with [`Burst::values`], which read nothing but the burst, so the same burst always says the
//! same.

use std::collections::HashMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use rd_core::{CategoryId, PackageId};

use crate::{NotificationEvent, Placeholder, Values};

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
    /// When the bus event happened.
    pub occurred_at: DateTime<Utc>,
    /// The started download's package; none for an intake.
    pub package_id: Option<PackageId>,
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
    /// When the bus event that opened the burst happened: the moment its `{time}` names.
    pub first_at: DateTime<Utc>,
    /// The package of the occurrence that opened the burst, for a burst of one start.
    pub package_id: Option<PackageId>,
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
                first_at: occurrence.occurred_at,
                package_id: occurrence.package_id,
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

impl Burst {
    /// The names as the text lists them: comma-separated, and for starts the downloads beyond
    /// them counted, `a.mkv, b.mkv, c.mkv and 47 more`. An intake's names are its sources, which
    /// are not counted.
    #[must_use]
    pub fn names_text(&self) -> String {
        let named = self.names.join(", ");
        if self.event != NotificationEvent::DownloadStarted {
            return named;
        }
        let listed = u64::try_from(self.names.len()).unwrap_or(u64::MAX);
        match self.items.saturating_sub(listed) {
            0 => named,
            rest => format!("{named} and {rest} more"),
        }
    }

    /// What a rule's own wording is filled with (RD-1291-01), the time and date those of the
    /// first occurrence in `zone`: the common placeholders but the budget's, `{count}`, `{items}`
    /// and `{names}`, and for a burst of one start its `{file}`. Its `{package}` and `{category}`
    /// need the database and are the service's.
    #[must_use]
    pub fn values<Tz: TimeZone>(&self, zone: &Tz) -> Values
    where
        Tz::Offset: std::fmt::Display,
    {
        let mut values = Values::stamped(self.event, &self.first_at.with_timezone(zone))
            .with(Placeholder::Count, self.occurrences.to_string())
            .with(Placeholder::Items, self.items.to_string())
            .with(Placeholder::Names, self.names_text());
        if self.single() && self.event == NotificationEvent::DownloadStarted {
            values.set(
                Placeholder::File,
                self.names.first().cloned().unwrap_or_default(),
            );
        }
        values
    }

    /// Whether the burst holds one occurrence only, which fills what a single event would.
    #[must_use]
    pub fn single(&self) -> bool {
        self.occurrences == 1
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone as _, Utc};
    use rd_core::CategoryId;

    use super::{Coalescer, NAMED, Occurrence};
    use crate::{NotificationEvent, Placeholder as P};

    fn links(id: &str, items: u64, name: &str) -> Occurrence {
        Occurrence {
            event: NotificationEvent::LinksAdded,
            category_id: None,
            event_id: id.to_owned(),
            items,
            name: Some(name.to_owned()),
            occurred_at: Utc::now(),
            package_id: None,
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
            occurred_at: start,
            package_id: None,
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

    /// RD-1291-01: what a rule's own wording reads from a closed burst.
    #[test]
    fn a_burst_fills_its_placeholders_and_one_start_its_file() {
        let at = Utc
            .with_ymd_and_hms(2026, 10, 14, 12, 5, 0)
            .single()
            .expect("moment");
        let started = |id: &str, name: &str| Occurrence {
            event: NotificationEvent::DownloadStarted,
            category_id: None,
            event_id: id.to_owned(),
            items: 1,
            name: Some(name.to_owned()),
            occurred_at: at,
            package_id: None,
        };
        let mut coalescer = Coalescer::default();
        coalescer.push(started("one", "a.mkv"), Utc::now());
        let one = coalescer.drain().remove(0);
        assert!(one.single());
        let values = one.values(&Utc);
        assert_eq!(values.get(P::Event), "download_started");
        assert_eq!(values.get(P::Time), "12:05");
        assert_eq!(values.get(P::Date), "2026-10-14");
        assert_eq!(values.get(P::Count), "1");
        assert_eq!(values.get(P::Items), "1");
        assert_eq!(values.get(P::Names), "a.mkv");
        assert_eq!(values.get(P::File), "a.mkv");

        for index in 0..5 {
            coalescer.push(
                started(&format!("e{index}"), &format!("part{index}.rar")),
                Utc::now(),
            );
        }
        let many = coalescer.drain().remove(0);
        let values = many.values(&Utc);
        assert_eq!(values.get(P::Count), "5");
        assert_eq!(values.get(P::Items), "5");
        assert_eq!(
            values.get(P::Names),
            "part0.rar, part1.rar, part2.rar and 2 more"
        );
        assert_eq!(values.get(P::File), "", "several starts name no one file");
        assert_eq!(values, many.values(&Utc), "the same burst says the same");

        // An intake's names are its sources, and its links are no file.
        for index in 0..4 {
            let source = if index % 2 == 0 { "clipboard" } else { "API" };
            coalescer.push(links(&format!("l{index}"), 3, source), Utc::now());
        }
        let imports = coalescer.drain().remove(0).values(&Utc);
        assert_eq!(imports.get(P::Count), "4");
        assert_eq!(imports.get(P::Items), "12");
        assert_eq!(imports.get(P::Names), "clipboard, API");
        assert_eq!(imports.get(P::File), "");
    }
}

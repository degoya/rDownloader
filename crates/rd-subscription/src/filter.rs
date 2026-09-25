//! Deciding whether a discovered item is wanted, and saying why when it is not
//! (RD-080-07).
//!
//! Every rejection carries the rule that caused it. That is not a nicety: a subscription
//! that produces nothing is otherwise indistinguishable from a broken filter, a broken
//! adapter, and a channel that simply has not posted. The reason is stored on the item, so
//! the answer survives to the next time somebody asks.
//!
//! The rules are evaluated in a fixed order and the *first* one that rejects wins, so the
//! reported reason is stable rather than dependent on evaluation order.

use chrono::{DateTime, Utc};
use rd_core::{BacklogPolicy, FilterReason, SubscriptionFilters};

/// What a poll learned about one item, in the vocabulary the filters speak.
#[derive(Clone, Debug, Default)]
pub struct CandidateItem {
    pub title: String,
    pub published_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<u32>,
    pub language: Option<String>,
    pub height: Option<u32>,
}

/// Whether an item is wanted, and if not, which rule said so.
pub type Decision = Result<(), FilterReason>;

/// Applies the backlog cutoff, then the filters.
///
/// `primed` is false only for the very first poll of a subscription; the backlog policy
/// applies exactly then and never again, because after that everything reaching this
/// function is genuinely new.
pub fn evaluate(
    item: &CandidateItem,
    filters: &SubscriptionFilters,
    backlog: BacklogPolicy,
    primed: bool,
    now: DateTime<Utc>,
) -> Decision {
    if !primed {
        backlog_decision(item, backlog, now)?;
    }
    filter_decision(item, filters)
}

/// The first poll's verdict on an item that already existed.
fn backlog_decision(item: &CandidateItem, backlog: BacklogPolicy, now: DateTime<Utc>) -> Decision {
    match backlog {
        // Everything present at activation is history. This is the default because the
        // alternative — a channel's whole decade of uploads entering the queue at once — is
        // the most destructive thing a subscription can do.
        BacklogPolicy::FromNow => {
            if item.published_at.is_none_or(|published| published <= now) {
                Err(FilterReason::Backlog)
            } else {
                Ok(())
            }
        }
        BacklogPolicy::Since(cutoff) => {
            if item.published_at.is_none_or(|published| published < cutoff) {
                Err(FilterReason::Backlog)
            } else {
                Ok(())
            }
        }
        // Collect it all for a person to look at; the caller keeps such items Pending
        // rather than queueing them, whatever the subscription's mode says.
        BacklogPolicy::ReviewAll => Ok(()),
    }
}

/// Whether a title pattern matches.
///
/// A pattern wrapped in slashes — `/^S0\d/` — is a regular expression; anything else is the
/// plain text it has always been. The distinction is explicit rather than inferred, because
/// most substrings people already wrote are also valid expressions with a different meaning:
/// `C++ (2026)` compiles happily as "one or more C, a space, then 2026" and stops matching the
/// thing it was written for. Guessing would rewrite existing subscriptions in silence.
///
/// An expression that does not compile matches nothing and says so to the caller, rather than
/// falling back to text: somebody who wrote slashes meant an expression.
#[must_use]
pub fn title_matches(pattern: &str, title_lowercase: &str) -> bool {
    match as_expression(pattern) {
        Some(expression) => regex::RegexBuilder::new(expression)
            .case_insensitive(true)
            .build()
            .is_ok_and(|expression| expression.is_match(title_lowercase)),
        None => !pattern.is_empty() && title_lowercase.contains(&pattern.to_lowercase()),
    }
}

/// The expression inside `/…/`, if the pattern is written that way.
#[must_use]
pub fn as_expression(pattern: &str) -> Option<&str> {
    let trimmed = pattern.trim();
    let inner = trimmed.strip_prefix('/')?.strip_suffix('/')?;
    (!inner.is_empty()).then_some(inner)
}

fn filter_decision(item: &CandidateItem, filters: &SubscriptionFilters) -> Decision {
    let title = item.title.to_lowercase();

    if !filters.title_contains.is_empty()
        && !filters
            .title_contains
            .iter()
            .any(|pattern| title_matches(pattern, &title))
    {
        return Err(FilterReason::TitleNotIncluded);
    }
    // Checked after the inclusion list so an exclusion always wins: "everything with
    // 'review' except the sponsored ones" is the shape people actually write.
    if filters
        .title_excludes
        .iter()
        .any(|pattern| title_matches(pattern, &title))
    {
        return Err(FilterReason::TitleExcluded);
    }
    // A missing duration is not a short item. Rejecting on absent metadata would silently
    // drop every source that does not report it, which is most feeds.
    if let (Some(minimum), Some(duration)) = (filters.min_duration_seconds, item.duration_seconds)
        && duration < minimum
    {
        return Err(FilterReason::TooShort);
    }
    if let (Some(maximum), Some(duration)) = (filters.max_duration_seconds, item.duration_seconds)
        && duration > maximum
    {
        return Err(FilterReason::TooLong);
    }
    if let (Some(cutoff), Some(published)) = (filters.published_after, item.published_at)
        && published < cutoff
    {
        return Err(FilterReason::TooOld);
    }
    if !filters.languages.is_empty()
        && let Some(language) = item.language.as_deref()
        && !filters
            .languages
            .iter()
            .any(|wanted| language_matches(wanted, language))
    {
        return Err(FilterReason::LanguageNotWanted);
    }
    if let (Some(minimum), Some(height)) = (filters.min_height, item.height)
        && height < minimum
    {
        return Err(FilterReason::ResolutionTooLow);
    }
    Ok(())
}

/// Compares on the primary subtag, so a wanted `en` accepts `en-GB` and `EN_US`.
fn language_matches(wanted: &str, actual: &str) -> bool {
    let primary = |value: &str| {
        value
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    primary(wanted) == primary(actual)
}

#[cfg(test)]
mod tests {
    use super::{CandidateItem, evaluate};
    use chrono::{Duration, Utc};
    use rd_core::{BacklogPolicy, FilterReason, SubscriptionFilters};

    fn item(title: &str) -> CandidateItem {
        CandidateItem {
            title: title.to_owned(),
            ..CandidateItem::default()
        }
    }

    /// Filters only, with the backlog gate already passed.
    fn check(item: &CandidateItem, filters: &SubscriptionFilters) -> super::Decision {
        evaluate(item, filters, BacklogPolicy::FromNow, true, Utc::now())
    }

    #[test]
    fn an_empty_filter_set_accepts_everything() {
        assert!(check(&item("anything at all"), &SubscriptionFilters::default()).is_ok());
    }

    #[test]
    fn the_inclusion_list_is_an_any_match_and_ignores_case() {
        let filters = SubscriptionFilters {
            title_contains: vec!["Review".to_owned(), "Teardown".to_owned()],
            ..SubscriptionFilters::default()
        };
        assert!(check(&item("Laptop review 2026"), &filters).is_ok());
        assert!(check(&item("Full TEARDOWN"), &filters).is_ok());
        assert_eq!(
            check(&item("Unboxing"), &filters),
            Err(FilterReason::TitleNotIncluded)
        );
    }

    #[test]
    fn an_exclusion_beats_an_inclusion() {
        // "everything with 'review' except the sponsored ones" is the shape people write.
        let filters = SubscriptionFilters {
            title_contains: vec!["review".to_owned()],
            title_excludes: vec!["sponsored".to_owned()],
            ..SubscriptionFilters::default()
        };
        assert!(check(&item("Honest review"), &filters).is_ok());
        assert_eq!(
            check(&item("Sponsored review"), &filters),
            Err(FilterReason::TitleExcluded)
        );
    }

    #[test]
    fn absent_metadata_never_rejects() {
        // Rejecting on missing data would silently drop every source that does not report
        // duration, language or resolution — which is most feeds.
        let filters = SubscriptionFilters {
            min_duration_seconds: Some(600),
            max_duration_seconds: Some(3_600),
            min_height: Some(1080),
            languages: vec!["en".to_owned()],
            ..SubscriptionFilters::default()
        };
        assert!(check(&item("No metadata"), &filters).is_ok());
    }

    #[test]
    fn duration_bounds_are_inclusive_at_the_edges() {
        let filters = SubscriptionFilters {
            min_duration_seconds: Some(600),
            max_duration_seconds: Some(3_600),
            ..SubscriptionFilters::default()
        };
        let with = |seconds| CandidateItem {
            duration_seconds: Some(seconds),
            ..item("x")
        };
        assert_eq!(check(&with(599), &filters), Err(FilterReason::TooShort));
        assert!(check(&with(600), &filters).is_ok());
        assert!(check(&with(3_600), &filters).is_ok());
        assert_eq!(check(&with(3_601), &filters), Err(FilterReason::TooLong));
    }

    #[test]
    fn a_language_matches_on_its_primary_subtag() {
        let filters = SubscriptionFilters {
            languages: vec!["en".to_owned()],
            ..SubscriptionFilters::default()
        };
        for tag in ["en", "en-GB", "EN_US"] {
            let candidate = CandidateItem {
                language: Some(tag.to_owned()),
                ..item("x")
            };
            assert!(check(&candidate, &filters).is_ok(), "{tag}");
        }
        let german = CandidateItem {
            language: Some("de".to_owned()),
            ..item("x")
        };
        assert_eq!(
            check(&german, &filters),
            Err(FilterReason::LanguageNotWanted)
        );
    }

    #[test]
    fn the_first_matching_rule_is_the_reported_one() {
        // Stable reasons matter: an item rejected for two things must not report a
        // different one on the next poll.
        let filters = SubscriptionFilters {
            title_excludes: vec!["trailer".to_owned()],
            min_duration_seconds: Some(600),
            ..SubscriptionFilters::default()
        };
        let candidate = CandidateItem {
            duration_seconds: Some(30),
            ..item("Trailer")
        };
        assert_eq!(
            check(&candidate, &filters),
            Err(FilterReason::TitleExcluded)
        );
    }

    #[test]
    fn the_first_poll_treats_existing_items_as_backlog() {
        let now = Utc::now();
        let existing = CandidateItem {
            published_at: Some(now - Duration::days(30)),
            ..item("Old upload")
        };
        assert_eq!(
            evaluate(
                &existing,
                &SubscriptionFilters::default(),
                BacklogPolicy::FromNow,
                false,
                now
            ),
            Err(FilterReason::Backlog)
        );
        // ... and the same item is accepted once the subscription is primed.
        assert!(
            evaluate(
                &existing,
                &SubscriptionFilters::default(),
                BacklogPolicy::FromNow,
                true,
                now
            )
            .is_ok()
        );
    }

    #[test]
    fn an_item_without_a_date_is_backlog_on_the_first_poll() {
        // Otherwise a feed that omits dates imports its entire history on activation.
        let now = Utc::now();
        assert_eq!(
            evaluate(
                &item("Undated"),
                &SubscriptionFilters::default(),
                BacklogPolicy::FromNow,
                false,
                now
            ),
            Err(FilterReason::Backlog)
        );
    }

    #[test]
    fn a_since_cutoff_admits_only_what_is_newer() {
        let now = Utc::now();
        let cutoff = now - Duration::days(7);
        let make = |age: i64| CandidateItem {
            published_at: Some(now - Duration::days(age)),
            ..item("x")
        };
        let decide = |candidate: &CandidateItem| {
            evaluate(
                candidate,
                &SubscriptionFilters::default(),
                BacklogPolicy::Since(cutoff),
                false,
                now,
            )
        };
        assert_eq!(decide(&make(30)), Err(FilterReason::Backlog));
        assert!(decide(&make(1)).is_ok());
    }

    #[test]
    fn review_all_lets_the_whole_backlog_through_for_a_person_to_see() {
        let now = Utc::now();
        let old = CandidateItem {
            published_at: Some(now - Duration::days(3_650)),
            ..item("Ancient")
        };
        assert!(
            evaluate(
                &old,
                &SubscriptionFilters::default(),
                BacklogPolicy::ReviewAll,
                false,
                now
            )
            .is_ok()
        );
    }
}

#[cfg(test)]
mod pattern_tests {
    use super::{as_expression, title_matches};

    #[test]
    fn a_pattern_in_slashes_is_a_regular_expression() {
        assert!(title_matches(r"/s0\d[.]e\d\d/", "show s01.e04 1080p"));
        assert!(!title_matches(r"/s0\d[.]e\d\d/", "show 2026 1080p"));
    }

    #[test]
    fn a_pattern_without_slashes_is_still_plain_text() {
        // The hazard this guards against: as an expression this compiles to "one or more C,
        // a space, then 2026" and stops matching what it was written for.
        assert!(title_matches("C++ (2026)", "learning c++ (2026) edition"));
        assert!(!title_matches("C++ (2026)", "learning rust"));
    }

    #[test]
    fn matching_ignores_case_either_way_round() {
        assert!(title_matches("REVIEW", "the review"));
        assert!(title_matches("/review/", "The REVIEW"));
    }

    #[test]
    fn an_expression_that_does_not_compile_matches_nothing() {
        // Somebody who wrote slashes meant an expression; matching the text between them
        // instead would quietly do something else.
        assert!(!title_matches("/a[/", "a[ literally"));
    }

    #[test]
    fn an_empty_pattern_matches_nothing() {
        assert!(!title_matches("", "anything at all"));
        assert!(!title_matches("//", "anything at all"));
    }

    #[test]
    fn the_delimiters_are_recognised_only_around_the_whole_pattern() {
        assert_eq!(as_expression("/abc/"), Some("abc"));
        assert_eq!(as_expression("  /abc/  "), Some("abc"));
        assert_eq!(as_expression("a/b/c"), None);
        assert_eq!(as_expression("/abc"), None);
    }
}

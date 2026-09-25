//! Who is around to answer a widget captcha, as far as the service can tell.
//!
//! The web interface cannot know whether a browser extension is installed: it runs in a
//! different browser, possibly on a different machine. What the service does know is when an
//! extension last polled the waiting widgets, because the poll names itself
//! (`?client=browser_extension`). That timestamp, and whether it is recent enough to count as
//! connected, is all this reports (RD-108-02).

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use utoipa::ToSchema;

/// How long after its last poll an extension still counts as connected: three of its
/// 30-second alarm ticks, so one missed poll does not flip the hint back and forth.
pub const BROWSER_EXTENSION_PRESENCE_WINDOW: Duration = Duration::seconds(90);

/// What the web interface may say about the ways a widget captcha can be answered.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct CaptchaAnswerers {
    /// When a browser extension last asked for the waiting widgets; absent until one has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_extension_seen_at: Option<DateTime<Utc>>,
    /// Whether that was recent enough to expect the extension to pick up a new challenge.
    pub browser_extension_connected: bool,
}

/// Turns the last poll into a verdict, so the rule lives in one place and is testable
/// without a clock.
pub(crate) fn answerers(seen_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> CaptchaAnswerers {
    CaptchaAnswerers {
        browser_extension_seen_at: seen_at,
        browser_extension_connected: seen_at
            .is_some_and(|seen| now - seen <= BROWSER_EXTENSION_PRESENCE_WINDOW),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use super::{BROWSER_EXTENSION_PRESENCE_WINDOW, answerers};

    #[test]
    fn nothing_has_polled_means_nothing_is_connected() {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 16, 12, 0, 0)
            .single()
            .expect("date");
        let verdict = answerers(None, now);
        assert!(verdict.browser_extension_seen_at.is_none());
        assert!(!verdict.browser_extension_connected);
    }

    /// One missed alarm tick must not turn the hint into "no extension": the window is three
    /// ticks wide, and a poll on the boundary still counts.
    #[test]
    fn a_recent_poll_counts_as_connected_and_an_old_one_does_not() {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 16, 12, 0, 0)
            .single()
            .expect("date");
        let recent = now - Duration::seconds(31);
        assert!(answerers(Some(recent), now).browser_extension_connected);

        let boundary = now - BROWSER_EXTENSION_PRESENCE_WINDOW;
        assert!(answerers(Some(boundary), now).browser_extension_connected);

        let stale = now - BROWSER_EXTENSION_PRESENCE_WINDOW - Duration::seconds(1);
        let verdict = answerers(Some(stale), now);
        assert!(!verdict.browser_extension_connected);
        assert_eq!(
            verdict.browser_extension_seen_at,
            Some(stale),
            "the timestamp is still reported so the interface can say how long ago"
        );
    }

    #[test]
    fn the_verdict_serialises_without_a_null_timestamp() {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 16, 12, 0, 0)
            .single()
            .expect("date");
        let json = serde_json::to_value(answerers(None, now)).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "browser_extension_connected": false })
        );
    }
}

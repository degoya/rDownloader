//! A login session, as the owner sees it in the session list.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::SessionId;

/// Hours without a request after which a session ends, unless the owner set otherwise.
pub const DEFAULT_SESSION_IDLE_HOURS: u32 = 12;
/// Hours from sign-in after which a session ends however busy it is: thirty days.
pub const DEFAULT_SESSION_MAX_HOURS: u32 = 720;
/// The idle limit the owner may choose, in hours: one hour to thirty days.
///
/// The floor sits far above the minute at which `last_used_at` is written, so the idle check
/// can never be decided by that resolution. There is no "never" (RD-130-09).
pub const SESSION_IDLE_HOURS_RANGE: std::ops::RangeInclusive<u32> = 1..=720;
/// The maximum lifetime the owner may choose, in hours: one hour to ninety days.
pub const SESSION_MAX_HOURS_RANGE: std::ops::RangeInclusive<u32> = 1..=2160;

/// How long a sign-in lasts: both an idle limit and a maximum lifetime (RD-130-09).
///
/// Sliding *and* absolute. Until 1.3 a session lived a fixed twelve hours from sign-in, which
/// ended it in the middle of an evening's work and still let a stolen cookie last the full
/// twelve hours. The idle limit ends a session nobody is using; the maximum lifetime is the
/// floor under how long a theft can last, because a session that is being used — by its owner
/// or by whoever took the cookie — would otherwise slide forever.
///
/// Both are checked when a session is read, not written into the row, so a shorter setting
/// ends the sessions that are already past it at once: a limit that only applied to the next
/// sign-in would protect nothing on the day somebody lowers it because of a lost laptop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    pub idle_hours: u32,
    pub max_hours: u32,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            idle_hours: DEFAULT_SESSION_IDLE_HOURS,
            max_hours: DEFAULT_SESSION_MAX_HOURS,
        }
    }
}

impl SessionLimits {
    /// Limits from stored values, a missing one taking its default and an out-of-range one
    /// its nearest bound.
    ///
    /// The settings route refuses out-of-range values, so the clamp only matters for a
    /// document edited by hand — where a zero would otherwise end every session on its first
    /// request, including the one somebody needs to put it right.
    #[must_use]
    pub fn clamped(idle_hours: Option<u32>, max_hours: Option<u32>) -> Self {
        let clamp = |value: u32, range: &std::ops::RangeInclusive<u32>| {
            value.clamp(*range.start(), *range.end())
        };
        Self {
            idle_hours: clamp(
                idle_hours.unwrap_or(DEFAULT_SESSION_IDLE_HOURS),
                &SESSION_IDLE_HOURS_RANGE,
            ),
            max_hours: clamp(
                max_hours.unwrap_or(DEFAULT_SESSION_MAX_HOURS),
                &SESSION_MAX_HOURS_RANGE,
            ),
        }
    }

    /// The idle limit as a duration.
    #[must_use]
    pub fn idle(&self) -> Duration {
        Duration::hours(i64::from(self.idle_hours))
    }

    /// The maximum lifetime as a duration.
    #[must_use]
    pub fn max(&self) -> Duration {
        Duration::hours(i64::from(self.max_hours))
    }

    /// When `session` ends under these limits if it is not used again.
    ///
    /// The earliest of three: the expiry stored at sign-in, which a *longer* setting leaves
    /// alone (the browser's cookie was issued for the old maximum, so extending the row would
    /// outlive the cookie anyway); the maximum lifetime counted from sign-in; and the idle
    /// limit counted from the last recorded use.
    #[must_use]
    pub fn ends_at(&self, session: &Session) -> DateTime<Utc> {
        session
            .expires_at
            .min(session.created_at + self.max())
            .min(session.last_used_at + self.idle())
    }
}

/// The longest user-agent string kept.
///
/// Enough to recognise a browser and an operating system, short enough that a client cannot
/// use the field as storage.
pub const MAX_USER_AGENT: usize = 200;

/// One session in the inventory. Never carries the bearer, only what it is for.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Session {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    /// When the session ends if it is not used again: the earliest of the expiry fixed at
    /// sign-in and the two limits in force now (see [`SessionLimits::ends_at`]).
    pub expires_at: DateTime<Utc>,
    /// The client's user agent, truncated, or `None` if it sent none.
    pub user_agent: Option<String>,
    /// Where the session was created from, as the proxy rules resolved it.
    pub client_ip: Option<String>,
    /// Whether this is the session making the request.
    ///
    /// Computed per request rather than stored: it is a property of who is asking, not of the
    /// session. It is what lets the interface offer "sign out everywhere else" without the
    /// caller having to work out which row is its own — and what stops that action from
    /// signing the caller out too.
    pub current: bool,
}

/// Shortens a user agent to what the inventory keeps.
#[must_use]
pub fn truncate_user_agent(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_USER_AGENT).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_or_blank_user_agent_is_none_rather_than_an_empty_string() {
        assert_eq!(truncate_user_agent(""), None);
        assert_eq!(truncate_user_agent("   "), None);
    }

    /// A client must not be able to use the field as storage.
    #[test]
    fn an_overlong_user_agent_is_truncated() {
        let long = "a".repeat(10_000);
        let kept = truncate_user_agent(&long).expect("kept");
        assert_eq!(kept.chars().count(), MAX_USER_AGENT);
    }

    fn at(hours_ago: i64) -> DateTime<Utc> {
        Utc::now() - Duration::hours(hours_ago)
    }

    fn session(
        created: DateTime<Utc>,
        last_used: DateTime<Utc>,
        expires: DateTime<Utc>,
    ) -> Session {
        Session {
            id: SessionId::new(),
            created_at: created,
            last_used_at: last_used,
            expires_at: expires,
            user_agent: None,
            client_ip: None,
            current: false,
        }
    }

    /// Each of the three bounds can be the one that ends a session.
    #[test]
    fn a_session_ends_at_the_earliest_of_its_three_bounds() {
        let limits = SessionLimits {
            idle_hours: 2,
            max_hours: 10,
        };
        let created = at(9);
        // Used two hours ago, so the idle limit ends it now — an hour before the maximum.
        let busy = session(created, at(2), created + Duration::hours(720));
        assert_eq!(
            limits.ends_at(&busy),
            busy.last_used_at + Duration::hours(2)
        );
        // Used a minute ago: now the maximum, counted from sign-in, comes first.
        let active = session(created, Utc::now(), created + Duration::hours(720));
        assert_eq!(limits.ends_at(&active), created + Duration::hours(10));
        // Signed in under a shorter maximum than today's: the stored expiry still holds.
        let older = session(created, Utc::now(), created + Duration::hours(4));
        assert_eq!(limits.ends_at(&older), older.expires_at);
    }

    /// A hand-edited zero must not end every session on its first request.
    #[test]
    fn stored_limits_are_clamped_into_range_and_default_when_missing() {
        assert_eq!(SessionLimits::clamped(None, None), SessionLimits::default());
        let clamped = SessionLimits::clamped(Some(0), Some(1_000_000));
        assert_eq!(clamped.idle_hours, *SESSION_IDLE_HOURS_RANGE.start());
        assert_eq!(clamped.max_hours, *SESSION_MAX_HOURS_RANGE.end());
    }

    /// The defaults are inside the ranges the settings route enforces.
    #[test]
    fn the_defaults_are_valid_settings() {
        assert!(SESSION_IDLE_HOURS_RANGE.contains(&DEFAULT_SESSION_IDLE_HOURS));
        assert!(SESSION_MAX_HOURS_RANGE.contains(&DEFAULT_SESSION_MAX_HOURS));
    }

    /// Truncation counts characters, not bytes, so it cannot split one in half.
    ///
    /// Three bytes per character, so a byte-based truncation would land inside one and panic
    /// rather than merely produce the wrong length. The character is CJK rather than an umlaut
    /// because `crates/rdownloader/tests/no_german.rs` rejects those in Rust sources, and a
    /// wider character tests the boundary harder anyway.
    #[test]
    fn truncation_does_not_cut_a_character_in_half() {
        let long = "日".repeat(10_000);
        let kept = truncate_user_agent(&long).expect("kept");
        assert_eq!(kept.chars().count(), MAX_USER_AGENT);
        assert!(kept.chars().all(|character| character == '日'));
    }
}

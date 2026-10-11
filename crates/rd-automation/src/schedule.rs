//! When a time-triggered automation is due (RD-1240-10).
//!
//! Pure arithmetic over an explicit clock and zone, like the subscription schedule: the cases
//! that matter — a restart a minute after a slot, a summer-time change, a slot missed by an
//! hour — are all about time and none of them are worth reproducing by waiting.
//!
//! Two rules the engine builds on:
//!
//! * **A slot is due for [`GRACE_SECONDS`] after its time, never longer.** The engine looks
//!   every few seconds, so a slot is seen within its window while the service runs; a slot the
//!   service was down for is not caught up after a restart (owner, 2026-10-10).
//! * **A slot has one stable name**, its wall-clock time in the service's zone. The run's
//!   idempotency key is built from it, so a slot seen again — after a restart inside the
//!   window, or in the hour a change from summer time repeats — never runs twice.

use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How long after its time a slot may still start a run, in seconds.
pub const GRACE_SECONDS: i64 = 120;
/// Longest cron expression accepted, in bytes; the subscription schedule's figure.
pub const MAX_CRON_LEN: usize = 120;
/// Longest interval, in minutes: one day. A longer rhythm is a cron expression.
pub const MAX_INTERVAL_MINUTES: u32 = 1_440;

/// When a time-triggered automation runs, read in the service's own time zone.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Schedule {
    /// Every `minutes` minutes, counted from midnight: `60` runs on the hour, `360` at 0, 6,
    /// 12 and 18 o'clock.
    Interval { minutes: u32 },
    /// A five-field cron expression (minute, hour, day of month, month, day of week) or an
    /// alias such as `@daily`, with POSIX weekdays.
    Cron { expression: String },
}

/// One due slot: the instant, and the wall-clock time that names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slot {
    pub at: DateTime<Utc>,
    pub local: NaiveDateTime,
}

impl Slot {
    /// The slot's name in the run history and its idempotency key.
    #[must_use]
    pub fn label(&self) -> String {
        self.local.format("%Y-%m-%dT%H:%M").to_string()
    }
}

/// Why a schedule cannot be stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleError(pub String);

impl Schedule {
    /// Checks the schedule names a time at all, read from `now` in `zone`.
    ///
    /// # Errors
    ///
    /// An interval outside 1 to [`MAX_INTERVAL_MINUTES`], a cron expression that does not
    /// parse, or one that names no future time (`0 0 30 2 *`).
    pub fn validate<Tz: TimeZone>(
        &self,
        now: DateTime<Utc>,
        zone: &Tz,
    ) -> Result<(), ScheduleError> {
        match self {
            Self::Interval { minutes } => {
                if (1..=MAX_INTERVAL_MINUTES).contains(minutes) {
                    Ok(())
                } else {
                    Err(ScheduleError(format!(
                        "interval must be 1 to {MAX_INTERVAL_MINUTES} minutes"
                    )))
                }
            }
            Self::Cron { expression } => {
                let cron = parse_cron(expression)?;
                cron.find_next_occurrence(&now.with_timezone(zone), false)
                    .map(|_| ())
                    .map_err(|error| {
                        ScheduleError(format!("schedule names no future time: {error}"))
                    })
            }
        }
    }

    /// The newest slot in the window `(now - GRACE_SECONDS, now]`, if there is one.
    #[must_use]
    pub fn due_slot<Tz: TimeZone>(&self, now: DateTime<Utc>, zone: &Tz) -> Option<Slot> {
        let window_start = now - Duration::seconds(GRACE_SECONDS);
        let slot = match self {
            Self::Interval { minutes } => interval_slot_at_or_before(*minutes, now, zone)?,
            Self::Cron { expression } => {
                let cron = parse_cron(expression).ok()?;
                let mut cursor = window_start.with_timezone(zone);
                let mut newest = None;
                // A five-field expression names at most one time per minute, so the window
                // holds at most three; the bound only guards against a parser surprise.
                for _ in 0..8 {
                    let Ok(next) = cron.find_next_occurrence(&cursor, false) else {
                        break;
                    };
                    if next.with_timezone(&Utc) > now {
                        break;
                    }
                    newest = Some(next.clone());
                    cursor = next;
                }
                let next = newest?;
                Slot {
                    at: next.with_timezone(&Utc),
                    local: next.naive_local(),
                }
            }
        };
        (slot.at > window_start && slot.at <= now).then_some(slot)
    }

    /// The first slot after `after`, for the editor's dry run.
    #[must_use]
    pub fn next_after<Tz: TimeZone>(
        &self,
        after: DateTime<Utc>,
        zone: &Tz,
    ) -> Option<DateTime<Utc>> {
        match self {
            Self::Interval { minutes } => {
                let step = Duration::minutes(i64::from((*minutes).max(1)));
                let mut local = after.with_timezone(zone).naive_local();
                local = floor_minutes(local, *minutes)?;
                // A slot inside a summer-time gap has no instant; the next one does.
                for _ in 0..4 {
                    local += step;
                    if let Some(at) = zone.from_local_datetime(&local).earliest() {
                        let at = at.with_timezone(&Utc);
                        if at > after {
                            return Some(at);
                        }
                    }
                }
                None
            }
            Self::Cron { expression } => parse_cron(expression)
                .ok()?
                .find_next_occurrence(&after.with_timezone(zone), false)
                .ok()
                .map(|next| next.with_timezone(&Utc)),
        }
    }
}

/// The interval slot at or before `now`: wall-clock minutes floored to the interval.
fn interval_slot_at_or_before<Tz: TimeZone>(
    minutes: u32,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Option<Slot> {
    let local = floor_minutes(now.with_timezone(zone).naive_local(), minutes)?;
    // A slot in the hour summer time skips does not exist and is not run; in the hour that
    // repeats, the first instant names it and the label keeps the second from running again.
    let at = zone
        .from_local_datetime(&local)
        .earliest()?
        .with_timezone(&Utc);
    Some(Slot { at, local })
}

/// Floors a wall-clock time to a multiple of `minutes` counted from midnight.
fn floor_minutes(local: NaiveDateTime, minutes: u32) -> Option<NaiveDateTime> {
    let minutes = i64::from(minutes.clamp(1, MAX_INTERVAL_MINUTES));
    let midnight = local.date().and_hms_opt(0, 0, 0)?;
    let since_midnight = (local - midnight).num_minutes();
    Some(midnight + Duration::minutes(since_midnight - since_midnight.rem_euclid(minutes)))
}

/// Reads a cron expression the way the subscription schedule does (RD-130-19): five fields
/// or an alias, no seconds, no years.
fn parse_cron(expression: &str) -> Result<croner::Cron, ScheduleError> {
    let expression = expression.trim();
    if expression.is_empty() || expression.len() > MAX_CRON_LEN {
        return Err(ScheduleError(format!(
            "schedule must be a cron expression of at most {MAX_CRON_LEN} bytes"
        )));
    }
    croner::parser::CronParser::builder()
        .seconds(croner::parser::Seconds::Disallowed)
        .year(croner::parser::Year::Disallowed)
        .build()
        .parse(expression)
        .map_err(|error| ScheduleError(format!("schedule is not a cron expression: {error}")))
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;

//! Dates, times and zones on `jiff`, byte-identical to chrono 0.4 wherever one is stored or sent
//! (RD-1440-05).
//!
//! The rule (owner, 2026-11-21): a value written before the switch and one written after are the
//! same bytes — JSON (REST, MCP, SSE) as chrono's serde, SQLite text as `sqlx`'s chrono
//! encoding, the OpenAPI document unchanged — so a database mixes no two text formats and a
//! `WHERE`/`ORDER BY` on a time column keeps ordering instants of the same second right.
//!
//! - [`Timestamp`] replaces `DateTime<Utc>`; [`Date`] replaces a stored or sent `NaiveDate`;
//!   [`IanaZone`] replaces a stored or sent `chrono_tz::Tz`.
//! - Everything only computed with stays plain jiff: `jiff::SignedDuration` for
//!   `chrono::Duration`, `jiff::civil::*`, `jiff::tz::TimeZone`, `jiff::ZonedDateTime`.

mod date;
mod error;
mod format;
mod parse;
mod schema;
#[cfg(any(feature = "sqlx", test))]
mod sqlite;
mod timestamp;
mod zone;

#[cfg(test)]
mod date_zone_tests;
#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod schema_tests;
#[cfg(test)]
mod sqlite_tests;
#[cfg(test)]
mod timestamp_tests;

pub use date::Date;
pub use error::TimeParseError;
pub use format::SecondsFormat;
pub use jiff::SignedDuration;
pub use timestamp::Timestamp;
pub use zone::{IanaZone, UnknownZone};

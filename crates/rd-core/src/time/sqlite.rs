//! The SQLite column impls of `Timestamp` and `Date` (feature `sqlx`), byte for byte what
//! `sqlx-sqlite` 0.9's chrono impls (`src/types/chrono.rs`) write and read.
//!
//! - `Timestamp` writes `to_rfc3339_opts(SecondsFormat::AutoSi, false)`, i.e. `…+00:00`. It
//!   reads TEXT in every form `Timestamp::parse_sqlite` knows, INTEGER as unix seconds and
//!   REAL as a Julian day number (`julianday(…)`).
//! - `Date` writes `%Y-%m-%d` and reads TEXT the way `NaiveDate::parse_from_str(…, "%F")` does.
//!
//! One deliberate difference: the declared type is `TEXT`, not chrono's `DATETIME`/`DATE` —
//! `sqlx` keeps those private. It shows only in a type-mismatch message; `compatible` accepts
//! the same column types chrono's did.

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};
use sqlx::{Decode, Encode, Type, TypeInfo, ValueRef};

use super::{Date, Timestamp};

impl Type<Sqlite> for Timestamp {
    fn type_info() -> SqliteTypeInfo {
        <str as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.name(), "DATETIME" | "TEXT" | "INTEGER" | "REAL")
    }
}

impl Encode<'_, Sqlite> for Timestamp {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        Encode::<Sqlite>::encode(self.to_rfc3339(), buf)
    }
}

impl<'r> Decode<'r, Sqlite> for Timestamp {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        // The value's own storage class; NULL never reaches here (`Option` handles it).
        let class = value.type_info().name().to_owned();
        match class.as_str() {
            "INTEGER" => {
                let seconds = <i64 as Decode<Sqlite>>::decode(value)?;
                Timestamp::from_unix_seconds(seconds)
                    .ok_or_else(|| format!("invalid datetime: {seconds}").into())
            }
            "REAL" => {
                let day = <f64 as Decode<Sqlite>>::decode(value)?;
                from_julian_day(day).ok_or_else(|| format!("invalid datetime: {day}").into())
            }
            _ => {
                let text = <&str as Decode<Sqlite>>::decode(value)?;
                Timestamp::parse_sqlite(text)
                    .map_err(|_| format!("invalid datetime: {text}").into())
            }
        }
    }
}

/// `sqlx`'s `decode_datetime_from_float`: seconds rounded toward negative infinity, the
/// remainder forward, so an instant before the epoch keeps its fraction on the right side.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the same lossy casts sqlx makes, after the same finiteness check"
)]
pub(super) fn from_julian_day(value: f64) -> Option<Timestamp> {
    const EPOCH_IN_JULIAN_DAYS: f64 = 2_440_587.5;
    const SECONDS_IN_DAY: f64 = 86_400.0;
    let timestamp = (value - EPOCH_IN_JULIAN_DAYS) * SECONDS_IN_DAY;
    if !timestamp.is_finite() {
        return None;
    }
    let seconds = timestamp.floor();
    let nanos = ((timestamp - seconds) * 1E9) as u32;
    Timestamp::from_unix(seconds as i64, nanos)
}

impl Type<Sqlite> for Date {
    fn type_info() -> SqliteTypeInfo {
        <str as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.name(), "DATE" | "TEXT")
    }
}

impl Encode<'_, Sqlite> for Date {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        Encode::<Sqlite>::encode(self.to_string(), buf)
    }
}

impl<'r> Decode<'r, Sqlite> for Date {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let text = <&str as Decode<Sqlite>>::decode(value)?;
        Ok(text.parse::<Date>()?)
    }
}

//! `Timestamp`: a UTC instant whose every representation is chrono's `DateTime<Utc>`.

use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::str::FromStr;

use jiff::SignedDuration;
use jiff::civil::{Date as CivilDate, DateTime as CivilDateTime};
use jiff::tz::{Offset, TimeZone};

use super::error::TimeParseError;
use super::format::{SecondsFormat, write_rfc3339};
use super::parse::{Grammar, parse_timestamp};

/// A UTC instant to the nanosecond — the one time type rDownloader stores and sends
/// (RD-1440-05). It replaces `chrono::DateTime<Utc>` with the same bytes everywhere:
///
/// - serde, `Display`, `Debug`: `2026-11-21T14:00:00.123Z` — chrono's
///   `to_rfc3339_opts(SecondsFormat::AutoSi, true)`; deserialization and `FromStr` accept
///   whatever chrono's `DateTime<Utc>: FromStr` accepts.
/// - SQLite (feature `sqlx`): `2026-11-21T14:00:00.123+00:00` — `sqlx`'s chrono encoding, the
///   same text as [`Self::to_rfc3339`]; decoding reads every text, integer and real form
///   `sqlx`'s chrono decode reads.
/// - OpenAPI: `{type: string, format: date-time}`; the derive recognises the name `Timestamp`
///   (utoipa's `jiff_0_2`), so name the type `Timestamp`, never an alias.
///
/// The range is jiff's, years -9999 to 9999; chrono reached further, which no stored value did.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(jiff::Timestamp);

impl Timestamp {
    /// 1970-01-01T00:00:00Z, also the `Default` (as chrono's).
    pub const UNIX_EPOCH: Self = Self(jiff::Timestamp::UNIX_EPOCH);
    /// The earliest instant, -009999-01-02T01:59:59Z.
    pub const MIN: Self = Self(jiff::Timestamp::MIN);
    /// The latest instant, 9999-12-30T22:00:00.999999999Z.
    pub const MAX: Self = Self(jiff::Timestamp::MAX);

    /// The current instant (chrono's `Utc::now()`).
    #[must_use]
    pub fn now() -> Self {
        Self(jiff::Timestamp::now())
    }

    /// Wraps a jiff instant.
    #[must_use]
    pub const fn from_jiff(inner: jiff::Timestamp) -> Self {
        Self(inner)
    }

    /// The jiff instant, for calendar and zone arithmetic.
    #[must_use]
    pub const fn as_jiff(self) -> jiff::Timestamp {
        self.0
    }

    /// `seconds` after the epoch plus `nanos` (chrono's `DateTime::from_timestamp`); `None`
    /// outside the range or for `nanos` above 999 999 999.
    #[must_use]
    pub fn from_unix(seconds: i64, nanos: u32) -> Option<Self> {
        if nanos >= 1_000_000_000 {
            return None;
        }
        let total = i128::from(seconds) * 1_000_000_000 + i128::from(nanos);
        jiff::Timestamp::from_nanosecond(total).ok().map(Self)
    }

    /// Whole seconds after the epoch (`DateTime::from_timestamp(seconds, 0)`).
    #[must_use]
    pub fn from_unix_seconds(seconds: i64) -> Option<Self> {
        Self::from_unix(seconds, 0)
    }

    /// Milliseconds after the epoch (`DateTime::from_timestamp_millis`).
    #[must_use]
    pub fn from_unix_millis(millis: i64) -> Option<Self> {
        jiff::Timestamp::from_nanosecond(i128::from(millis) * 1_000_000)
            .ok()
            .map(Self)
    }

    /// Seconds after the epoch, rounded down (chrono's `timestamp()`: -0.5 s is -1).
    #[must_use]
    pub fn unix_seconds(self) -> i64 {
        let floor = self.0.as_nanosecond().div_euclid(1_000_000_000);
        i64::try_from(floor).unwrap_or(if floor < 0 { i64::MIN } else { i64::MAX })
    }

    /// Milliseconds after the epoch, rounded down (chrono's `timestamp_millis()`).
    #[must_use]
    pub fn unix_millis(self) -> i64 {
        let floor = self.0.as_nanosecond().div_euclid(1_000_000);
        i64::try_from(floor).unwrap_or(if floor < 0 { i64::MIN } else { i64::MAX })
    }

    /// Nanoseconds after the epoch.
    #[must_use]
    pub fn unix_nanos(self) -> i128 {
        self.0.as_nanosecond()
    }

    /// The fraction of the second, 0..=999 999 999 (chrono's `timestamp_subsec_nanos()`).
    #[must_use]
    pub fn subsec_nanos(self) -> u32 {
        let fraction = self.0.as_nanosecond().rem_euclid(1_000_000_000);
        u32::try_from(fraction).unwrap_or(0)
    }

    /// The fraction in whole milliseconds (chrono's `timestamp_subsec_millis()`).
    #[must_use]
    pub fn subsec_millis(self) -> u32 {
        self.subsec_nanos() / 1_000_000
    }

    /// Parses RFC 3339 strictly, any offset converted to UTC — chrono's
    /// `DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc)`.
    ///
    /// # Errors
    /// When `s` is not RFC 3339 or out of range.
    pub fn parse_rfc3339(s: &str) -> Result<Self, TimeParseError> {
        parse_timestamp(s, Grammar::Rfc3339).map(Self)
    }

    /// Parses the forms `sqlx`'s chrono decode reads from a SQLite text column.
    ///
    /// # Errors
    /// When `s` is none of them.
    pub fn parse_sqlite(s: &str) -> Result<Self, TimeParseError> {
        parse_timestamp(s, Grammar::Sqlite).map(Self)
    }

    /// RFC 3339 with `+00:00` and 0, 3, 6 or 9 fraction digits — chrono's `to_rfc3339()`, and
    /// the text a `Timestamp` column holds.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        self.to_rfc3339_opts(SecondsFormat::AutoSi, false)
    }

    /// RFC 3339 with the fraction `form` and `Z` (`use_z`) or `+00:00` — chrono's
    /// `to_rfc3339_opts`.
    #[must_use]
    pub fn to_rfc3339_opts(self, form: SecondsFormat, use_z: bool) -> String {
        let mut text = String::with_capacity(32);
        // Writing into a `String` cannot fail.
        let _ = write_rfc3339(&mut text, self.0, form, use_z);
        text
    }

    /// `strftime` in UTC (chrono's `format(…)`): jiff's directives, which match chrono's for
    /// `%Y %m %d %H %M %S %b %-d %3f %.9f %z %Z`; `%.f` differs (jiff trims, chrono writes
    /// 0/3/6/9 digits), so it is not used for stored text.
    #[must_use]
    pub fn strftime<'f, F: 'f + ?Sized + AsRef<[u8]>>(
        &self,
        format: &'f F,
    ) -> jiff::fmt::strtime::Display<'f> {
        self.0.strftime(format)
    }

    /// The UTC calendar date (chrono's `date_naive()`).
    #[must_use]
    pub fn date(self) -> CivilDate {
        self.utc_datetime().date()
    }

    /// The UTC civil date and time (chrono's `naive_utc()`).
    #[must_use]
    pub fn utc_datetime(self) -> CivilDateTime {
        Offset::UTC.to_datetime(self.0)
    }

    /// A UTC civil date and time as an instant (chrono's `naive.and_utc()`); saturates at the
    /// ends of the range, which no civil value reaches.
    #[must_use]
    pub fn from_utc_datetime(datetime: CivilDateTime) -> Self {
        match Offset::UTC.to_timestamp(datetime) {
            Ok(inner) => Self(inner),
            Err(_) if datetime.year() < 0 => Self::MIN,
            Err(_) => Self::MAX,
        }
    }

    /// `Utc.with_ymd_and_hms(…).single()`: `None` for a date or time that does not exist.
    #[must_use]
    pub fn from_ymd_hms(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Option<Self> {
        let datetime = CivilDateTime::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
            i8::try_from(hour).ok()?,
            i8::try_from(minute).ok()?,
            i8::try_from(second).ok()?,
            0,
        )
        .ok()?;
        Some(Self::from_utc_datetime(datetime))
    }

    /// The instant in `zone` (chrono's `with_timezone(&tz)`).
    #[must_use]
    pub fn to_zoned(self, zone: TimeZone) -> jiff::Zoned {
        self.0.to_zoned(zone)
    }

    /// `self - earlier` (chrono's `signed_duration_since`).
    #[must_use]
    pub fn duration_since(self, earlier: Self) -> SignedDuration {
        self.0.duration_since(earlier.0)
    }

    /// `self + duration`, `None` past the range (chrono's `checked_add_signed`).
    #[must_use]
    pub fn checked_add(self, duration: SignedDuration) -> Option<Self> {
        self.0.checked_add(duration).ok().map(Self)
    }

    /// `self - duration`, `None` past the range (chrono's `checked_sub_signed`).
    #[must_use]
    pub fn checked_sub(self, duration: SignedDuration) -> Option<Self> {
        self.0.checked_sub(duration).ok().map(Self)
    }

    /// `self + duration`, held at [`Self::MIN`] or [`Self::MAX`].
    #[must_use]
    pub fn saturating_add(self, duration: SignedDuration) -> Self {
        self.checked_add(duration)
            .unwrap_or(if duration.is_negative() {
                Self::MIN
            } else {
                Self::MAX
            })
    }

    /// `self - duration`, held at [`Self::MIN`] or [`Self::MAX`].
    #[must_use]
    pub fn saturating_sub(self, duration: SignedDuration) -> Self {
        self.checked_sub(duration)
            .unwrap_or(if duration.is_negative() {
                Self::MAX
            } else {
                Self::MIN
            })
    }

    /// Drops the fraction past `digits` (0..=9) digits, rounding down in time — chrono's
    /// `trunc_subsecs`; `trunc_subsecs(0)` is `with_nanosecond(0)`.
    #[must_use]
    pub fn trunc_subsecs(self, digits: u16) -> Self {
        let Some(step) = 9u32
            .checked_sub(u32::from(digits))
            .map(|exponent| 10i128.pow(exponent))
        else {
            return self;
        };
        let total = self.0.as_nanosecond();
        jiff::Timestamp::from_nanosecond(total - total.rem_euclid(step))
            .map(Self)
            .unwrap_or(self)
    }
}

impl From<jiff::Timestamp> for Timestamp {
    fn from(inner: jiff::Timestamp) -> Self {
        Self(inner)
    }
}

impl From<Timestamp> for jiff::Timestamp {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}

impl From<&jiff::Zoned> for Timestamp {
    fn from(zoned: &jiff::Zoned) -> Self {
        Self(zoned.timestamp())
    }
}

impl From<jiff::Zoned> for Timestamp {
    fn from(zoned: jiff::Zoned) -> Self {
        Self(zoned.timestamp())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_rfc3339(f, self.0, SecondsFormat::AutoSi, true)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Timestamp {
    type Err = TimeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_timestamp(s, Grammar::Relaxed).map(Self)
    }
}

impl serde::Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Timestamp;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an RFC 3339 formatted date and time string")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Timestamp, E> {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

/// `+` panics past the range, as chrono's did; [`Timestamp::checked_add`] does not.
impl Add<SignedDuration> for Timestamp {
    type Output = Self;

    fn add(self, duration: SignedDuration) -> Self {
        Self(self.0 + duration)
    }
}

impl Sub<SignedDuration> for Timestamp {
    type Output = Self;

    fn sub(self, duration: SignedDuration) -> Self {
        Self(self.0 - duration)
    }
}

impl AddAssign<SignedDuration> for Timestamp {
    fn add_assign(&mut self, duration: SignedDuration) {
        *self = *self + duration;
    }
}

impl SubAssign<SignedDuration> for Timestamp {
    fn sub_assign(&mut self, duration: SignedDuration) {
        *self = *self - duration;
    }
}

/// `later - earlier`, as chrono's `DateTime - DateTime`.
impl Sub for Timestamp {
    type Output = SignedDuration;

    fn sub(self, earlier: Self) -> SignedDuration {
        self.duration_since(earlier)
    }
}

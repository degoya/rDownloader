//! `Date`: a calendar day whose every representation is chrono's `NaiveDate`.

use std::fmt;
use std::str::FromStr;

use jiff::civil::Date as CivilDate;

use super::error::TimeParseError;
use super::format::write_date;
use super::parse::parse_date;

/// A calendar day without a zone, for the dates rDownloader stores and sends (a quota's reset
/// day, a site rule's check date) — `chrono::NaiveDate` with the same bytes (RD-1440-05):
/// serde, `Display` and the SQLite text are `%Y-%m-%d`; parsing accepts what chrono's
/// `NaiveDate: FromStr` accepts (one-digit fields, surrounding whitespace). OpenAPI renders the
/// name `Date` as `{type: string, format: date}`, as it did `NaiveDate`. Arithmetic and
/// weekdays: [`Self::as_jiff`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(CivilDate);

impl Date {
    /// `NaiveDate::from_ymd_opt`.
    #[must_use]
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Option<Self> {
        CivilDate::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()
        .map(Self)
    }

    /// Wraps a jiff date.
    #[must_use]
    pub const fn from_jiff(inner: CivilDate) -> Self {
        Self(inner)
    }

    /// The jiff date.
    #[must_use]
    pub const fn as_jiff(self) -> CivilDate {
        self.0
    }
}

impl Default for Date {
    /// 1970-01-01, as chrono's.
    fn default() -> Self {
        Self(CivilDate::constant(1970, 1, 1))
    }
}

impl From<CivilDate> for Date {
    fn from(inner: CivilDate) -> Self {
        Self(inner)
    }
}

impl From<Date> for CivilDate {
    fn from(value: Date) -> Self {
        value.0
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_date(f, self.0)
    }
}

impl fmt::Debug for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Date {
    type Err = TimeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_date(s).map(Self)
    }
}

impl serde::Serialize for Date {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Date {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Date;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a formatted date string")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Date, E> {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

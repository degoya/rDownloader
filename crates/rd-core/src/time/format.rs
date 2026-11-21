//! The RFC 3339 writer: chrono 0.4's `write_rfc3339`, digit for digit.

use std::fmt::{self, Write};

use jiff::civil::Date as CivilDate;
use jiff::tz::Offset;

/// How many fraction digits [`Timestamp::to_rfc3339_opts`](super::Timestamp::to_rfc3339_opts)
/// writes — chrono's `SecondsFormat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecondsFormat {
    /// No fraction: `…T14:00:00Z`.
    Secs,
    /// Always three digits: `…T14:00:00.000Z`.
    Millis,
    /// Always six digits.
    Micros,
    /// Always nine digits.
    Nanos,
    /// The fewest of 0, 3, 6 or 9 digits that lose nothing — what serde and SQLite write.
    AutoSi,
}

/// Writes `ts` in UTC as RFC 3339; `use_z` picks `Z` over `+00:00`.
pub(super) fn write_rfc3339(
    w: &mut impl Write,
    ts: jiff::Timestamp,
    form: SecondsFormat,
    use_z: bool,
) -> fmt::Result {
    let dt = Offset::UTC.to_datetime(ts);
    write_date(w, dt.date())?;
    write!(w, "T{:02}:{:02}:{:02}", dt.hour(), dt.minute(), dt.second())?;
    // A civil time's fraction is never negative.
    let nano = u32::try_from(dt.subsec_nanosecond()).unwrap_or(0);
    match form {
        SecondsFormat::Secs => {}
        SecondsFormat::AutoSi if nano == 0 => {}
        SecondsFormat::Millis => write!(w, ".{:03}", nano / 1_000_000)?,
        SecondsFormat::AutoSi if nano % 1_000_000 == 0 => write!(w, ".{:03}", nano / 1_000_000)?,
        SecondsFormat::Micros => write!(w, ".{:06}", nano / 1_000)?,
        SecondsFormat::AutoSi if nano % 1_000 == 0 => write!(w, ".{:06}", nano / 1_000)?,
        SecondsFormat::Nanos | SecondsFormat::AutoSi => write!(w, ".{nano:09}")?,
    }
    w.write_str(if use_z { "Z" } else { "+00:00" })
}

/// `%Y-%m-%d` with chrono's rule for the year: four digits in 0..=9999, otherwise signed.
pub(super) fn write_date(w: &mut impl Write, date: CivilDate) -> fmt::Result {
    let year = date.year();
    if (0..=9999).contains(&year) {
        write!(w, "{year:04}")?;
    } else {
        write!(w, "{year:+05}")?;
    }
    write!(w, "-{:02}-{:02}", date.month(), date.day())
}

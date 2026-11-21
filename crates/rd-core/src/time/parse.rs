//! The parsers: chrono 0.4's three grammars for a date and time, and its date grammar.
//!
//! - [`Grammar::Rfc3339`] is `DateTime::parse_from_rfc3339` (`format/parse.rs`, `parse_rfc3339`):
//!   fixed widths, `T`, `t` or a space between date and time, `Z`/`z` or `±HH:MM`.
//! - [`Grammar::Relaxed`] is `DateTime<Utc>: FromStr` and so serde (`parse_rfc3339_relaxed`):
//!   whitespace around every field, one-digit fields, signed years, `UTC`, `±HHMM`, `±HH MM`.
//! - [`Grammar::Sqlite`] is `sqlx-sqlite`'s chrono decode: RFC 3339 first, then the SQLite
//!   formats `%F %R`, `%F %T%.f`, each with `T` or a space, with `Z`, `%:z` or no offset (UTC).
//!   It accepts the relaxed grammar with the seconds and the offset optional — a superset.
//!
//! A leap second (`:60`) is clamped to `:59.999999999`: jiff has none, chrono kept it as a
//! fraction above one second at `:59`, so the order and the unix seconds stay the same.

use jiff::civil::{Date as CivilDate, DateTime, Time};
use jiff::tz::Offset;

use super::error::{INVALID, OUT_OF_RANGE, TOO_LONG, TOO_SHORT, TimeParseError};

type Step<'a, T> = Result<(&'a str, T), TimeParseError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Grammar {
    Rfc3339,
    Relaxed,
    Sqlite,
}

/// Parses an instant in `grammar`; the whole input must be consumed.
pub(super) fn parse_timestamp(
    input: &str,
    grammar: Grammar,
) -> Result<jiff::Timestamp, TimeParseError> {
    match grammar {
        Grammar::Rfc3339 => strict(input),
        Grammar::Relaxed => relaxed(input, false),
        Grammar::Sqlite => strict(input).or_else(|_| relaxed(input, true)),
    }
}

/// Parses `%Y-%m-%d` the way chrono's `NaiveDate: FromStr` does.
pub(super) fn parse_date(input: &str) -> Result<CivilDate, TimeParseError> {
    let (rest, (year, month, day)) = date_fields(input)?;
    if !rest.trim_start().is_empty() {
        return Err(TOO_LONG);
    }
    civil_date(year, month, day)
}

fn strict(s: &str) -> Result<jiff::Timestamp, TimeParseError> {
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return Err(TOO_SHORT);
    }
    let digit = |index: usize| -> Result<i64, TimeParseError> {
        match bytes[index] {
            b @ b'0'..=b'9' => Ok(i64::from(b - b'0')),
            _ => Err(INVALID),
        }
    };
    let pair = |index: usize| -> Result<i64, TimeParseError> {
        Ok(digit(index)? * 10 + digit(index + 1)?)
    };
    let expect = |index: usize, wanted: &'static [u8]| -> Result<(), TimeParseError> {
        if wanted.contains(&bytes[index]) {
            Ok(())
        } else {
            Err(INVALID)
        }
    };
    let year = pair(0)? * 100 + pair(2)?;
    expect(4, b"-".as_slice())?;
    let month = pair(5)?;
    expect(7, b"-".as_slice())?;
    let day = pair(8)?;
    let date = civil_date(year, month, day)?;
    expect(10, b"tT ".as_slice())?;
    let hour = pair(11)?;
    expect(13, b":".as_slice())?;
    let minute = pair(14)?;
    expect(16, b":".as_slice())?;
    let second = pair(17)?;
    let (rest, nano) = match s[19..].strip_prefix('.') {
        Some(fraction) => nanosecond(fraction)?,
        None => (&s[19..], 0),
    };
    let (rest, offset) = timezone_offset(rest, strict_colon)?;
    if !rest.is_empty() {
        return Err(TOO_LONG);
    }
    instant(date, hour, minute, second, nano, offset)
}

fn relaxed(s: &str, sqlite: bool) -> Result<jiff::Timestamp, TimeParseError> {
    let (s, (year, month, day)) = date_fields(s)?;
    let date = civil_date(year, month, day)?;
    let s = match s.as_bytes().first() {
        Some(b't' | b'T' | b' ') => &s[1..],
        Some(_) => return Err(INVALID),
        None => return Err(TOO_SHORT),
    };
    let (s, hour) = number(s.trim_start(), 1, 2)?;
    let (s, minute) = number(literal(s.trim_start(), ':')?.trim_start(), 1, 2)?;
    // `%F %R` has no seconds; serde's grammar requires them.
    let (s, second, nano) = match (sqlite, s.trim_start().strip_prefix(':')) {
        (true, None) => (s, 0, 0),
        (_, colon) => {
            let after = match colon {
                Some(after) => after,
                None => literal(s.trim_start(), ':')?,
            };
            let (s, second) = number(after.trim_start(), 1, 2)?;
            let (s, nano) = match s.strip_prefix('.') {
                Some(fraction) => nanosecond(fraction)?,
                None => (s, 0),
            };
            (s, second, nano)
        }
    };
    let s = s.trim_start();
    let (s, offset) = if s.len() >= 3 && s.as_bytes()[..3].eq_ignore_ascii_case(b"UTC") {
        (&s[3..], 0)
    } else if sqlite && s.is_empty() {
        // `NaiveDateTime::parse_from_str` and then UTC: a SQLite text without an offset.
        (s, 0)
    } else {
        timezone_offset(s, colon_or_space)?
    };
    if !s.trim_start().is_empty() {
        return Err(TOO_LONG);
    }
    instant(date, hour, minute, second, nano, offset)
}

/// Year, `-`, month, `-`, day, whitespace allowed around each (chrono's `Numeric` and `Space`).
fn date_fields(s: &str) -> Step<'_, (i64, i64, i64)> {
    let (s, year) = signed_year(s.trim_start())?;
    let (s, month) = number(literal(s.trim_start(), '-')?.trim_start(), 1, 2)?;
    let (s, day) = number(literal(s.trim_start(), '-')?.trim_start(), 1, 2)?;
    Ok((s, (year, month, day)))
}

fn signed_year(s: &str) -> Step<'_, i64> {
    if let Some(rest) = s.strip_prefix('-') {
        let (rest, value) = number(rest, 1, usize::MAX)?;
        Ok((rest, -value))
    } else if let Some(rest) = s.strip_prefix('+') {
        number(rest, 1, usize::MAX)
    } else {
        number(s, 1, 4)
    }
}

fn literal(s: &str, wanted: char) -> Result<&str, TimeParseError> {
    match s.strip_prefix(wanted) {
        Some(rest) => Ok(rest),
        None if s.is_empty() => Err(TOO_SHORT),
        None => Err(INVALID),
    }
}

/// chrono's `scan::number`: `min..=max` ASCII digits.
fn number(s: &str, min: usize, max: usize) -> Step<'_, i64> {
    let bytes = s.as_bytes();
    if bytes.len() < min {
        return Err(TOO_SHORT);
    }
    let mut value = 0i64;
    for (index, byte) in bytes.iter().take(max).enumerate() {
        if !byte.is_ascii_digit() {
            return if index < min {
                Err(INVALID)
            } else {
                Ok((&s[index..], value))
            };
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(i64::from(byte - b'0')))
            .ok_or(OUT_OF_RANGE)?;
    }
    Ok((&s[max.min(bytes.len())..], value))
}

/// chrono's `scan::nanosecond`: one to nine digits count, further digits are skipped.
fn nanosecond(s: &str) -> Step<'_, i64> {
    const SCALE: [i64; 10] = [
        0,
        100_000_000,
        10_000_000,
        1_000_000,
        100_000,
        10_000,
        1_000,
        100,
        10,
        1,
    ];
    let (rest, value) = number(s, 1, 9)?;
    let consumed = s.len() - rest.len();
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    Ok((rest, value * SCALE[consumed]))
}

fn strict_colon(s: &str) -> Result<&str, TimeParseError> {
    literal(s, ':')
}

fn colon_or_space(s: &str) -> Result<&str, TimeParseError> {
    Ok(s.trim_start_matches(|c: char| c == ':' || c.is_whitespace()))
}

/// chrono's `scan::timezone_offset` with `Z` allowed, minutes required, U+2212 accepted.
fn timezone_offset(s: &str, colon: impl Fn(&str) -> Result<&str, TimeParseError>) -> Step<'_, i64> {
    if let Some(b'Z' | b'z') = s.as_bytes().first() {
        return Ok((&s[1..], 0));
    }
    let (s, negative) = if let Some(rest) = s.strip_prefix('+') {
        (rest, false)
    } else if let Some(rest) = s.strip_prefix('-') {
        (rest, true)
    } else if let Some(rest) = s.strip_prefix('\u{2212}') {
        (rest, true)
    } else if s.is_empty() {
        return Err(TOO_SHORT);
    } else {
        return Err(INVALID);
    };
    let two = |s: &str| -> Result<(u8, u8), TimeParseError> {
        match s.as_bytes() {
            [first, second, ..] => Ok((*first, *second)),
            _ => Err(TOO_SHORT),
        }
    };
    let hours = match two(s)? {
        (h1 @ b'0'..=b'9', h2 @ b'0'..=b'9') => i64::from((h1 - b'0') * 10 + (h2 - b'0')),
        _ => return Err(INVALID),
    };
    let s = colon(&s[2..])?;
    let minutes = match two(s)? {
        (m1 @ b'0'..=b'5', m2 @ b'0'..=b'9') => i64::from((m1 - b'0') * 10 + (m2 - b'0')),
        (b'6'..=b'9', b'0'..=b'9') => return Err(OUT_OF_RANGE),
        _ => return Err(INVALID),
    };
    let seconds = hours * 3600 + minutes * 60;
    Ok((&s[2..], if negative { -seconds } else { seconds }))
}

fn civil_date(year: i64, month: i64, day: i64) -> Result<CivilDate, TimeParseError> {
    let narrow = |value: i64| i8::try_from(value).map_err(|_| OUT_OF_RANGE);
    let year = i16::try_from(year).map_err(|_| OUT_OF_RANGE)?;
    CivilDate::new(year, narrow(month)?, narrow(day)?).map_err(|_| OUT_OF_RANGE)
}

fn instant(
    date: CivilDate,
    hour: i64,
    minute: i64,
    second: i64,
    nano: i64,
    offset: i64,
) -> Result<jiff::Timestamp, TimeParseError> {
    let (second, nano) = if second == 60 {
        (59, 999_999_999)
    } else {
        (second, nano)
    };
    let narrow = |value: i64| i8::try_from(value).map_err(|_| OUT_OF_RANGE);
    let nano = i32::try_from(nano).map_err(|_| OUT_OF_RANGE)?;
    let time = Time::new(narrow(hour)?, narrow(minute)?, narrow(second)?, nano)
        .map_err(|_| OUT_OF_RANGE)?;
    // chrono's `FixedOffset::east_opt`: strictly inside one day.
    if offset.abs() >= 86_400 {
        return Err(OUT_OF_RANGE);
    }
    let offset = i32::try_from(offset).map_err(|_| OUT_OF_RANGE)?;
    let offset = Offset::from_seconds(offset).map_err(|_| OUT_OF_RANGE)?;
    offset
        .to_timestamp(DateTime::from_parts(date, time))
        .map_err(|_| OUT_OF_RANGE)
}

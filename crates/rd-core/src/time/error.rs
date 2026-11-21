//! The one parse error of the time types.

use std::fmt;

/// Why a date, a date and time or a zone name did not parse.
///
/// The messages are chrono 0.4's `ParseError` texts word for word, so an API error that quotes
/// one reads as it did before RD-1440-05.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimeParseError(pub(super) Kind);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Kind {
    OutOfRange,
    Invalid,
    TooShort,
    TooLong,
}

impl fmt::Display for TimeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Kind::OutOfRange => "input is out of range",
            Kind::Invalid => "input contains invalid characters",
            Kind::TooShort => "premature end of input",
            Kind::TooLong => "trailing input",
        })
    }
}

impl std::error::Error for TimeParseError {}

pub(super) const OUT_OF_RANGE: TimeParseError = TimeParseError(Kind::OutOfRange);
pub(super) const INVALID: TimeParseError = TimeParseError(Kind::Invalid);
pub(super) const TOO_SHORT: TimeParseError = TimeParseError(Kind::TooShort);
pub(super) const TOO_LONG: TimeParseError = TimeParseError(Kind::TooLong);

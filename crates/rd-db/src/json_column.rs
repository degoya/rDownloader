//! The lenient read of a stored JSON column (audit Q1, 2026-10-05).
//!
//! Many columns fall back to a default on purpose when their blob no longer parses: a value a
//! newer version wrote, or a damaged one, must not hide the row it belongs to. The fallback
//! stays, but it is not silent any more — every unreadable value leaves a warning naming the
//! table, the column and the row, so a setting that quietly became its default can be found.

use std::fmt::Display;

/// The parsed value, or `None` with a warning when it did not parse.
///
/// Takes the parse's result rather than the text, so the same helper serves `from_str`,
/// `from_value` and the quoted-enum spelling alike; the caller keeps its own default.
pub(crate) fn lenient<T>(
    parsed: serde_json::Result<T>,
    table: &str,
    column: &str,
    row: impl Display,
) -> Option<T> {
    match parsed {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(
                table,
                column,
                row = %row,
                %error,
                "a stored JSON value is unreadable and reads as its default"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "json_column_tests.rs"]
mod tests;

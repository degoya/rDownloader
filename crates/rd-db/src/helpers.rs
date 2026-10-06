//! Conversions between domain values and the text the tables store them as.

use std::str::FromStr;

use anyhow::{Context, Result};

/// Converts a stored textual identifier into a domain identifier.
pub(crate) fn parse_id<T>(value: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse::<T>().context("parse stored identifier")
}

/// Stores a serde enum as the bare string its JSON form quotes (`"queued"` -> `queued`).
pub(crate) fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}

/// An instant as the audit, log, collision and storage-operation tables store it: RFC 3339 in
/// UTC with milliseconds, so the text sorts the way the instants do.
pub(crate) fn timestamp(value: &chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `LIKE` treats `%` and `_` as wildcards; a person searching for `100%` means the characters.
pub(crate) fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Reads back an instant [`timestamp`] stored.
pub(crate) fn parse_time(value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    Ok(chrono::DateTime::parse_from_rfc3339(value)
        .context("parse stored timestamp")?
        .with_timezone(&chrono::Utc))
}

/// Reads back a value [`enum_string`] stored.
pub(crate) fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).context("parse stored enum")
}

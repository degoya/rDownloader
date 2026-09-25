//! What Google said no with, reduced to something safe to repeat.
//!
//! Shared because the rule has to be the same everywhere it is applied, and because getting it
//! wrong is silent: a Drive error document carries a `message` written for a developer, and an
//! endpoint that echoed part of a token into one would publish it through a log line and
//! through the failure the interface shows.
//!
//! The rule is RD-105-01's, with one difference. `sanitize_error` in the OAuth template accepts
//! RFC 6749 error codes — lowercase letters and underscores — because that is what a token
//! endpoint sends. Google's API reasons are lowerCamelCase (`downloadQuotaExceeded`) or
//! SCREAMING_SNAKE (`RESOURCE_EXHAUSTED`), so the accepted shape here is ASCII letters, digits
//! and underscores. What does *not* change is the important half: a value that is not that
//! shape is dropped **whole** rather than filtered character by character, because filtering
//! keeps the digits of a leaked token.

use serde::Deserialize;

/// The error document Google returns for every refusal, at every endpoint.
#[derive(Debug, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub code: Option<u16>,
    #[serde(default)]
    pub errors: Vec<ErrorDetail>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorDetail {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

/// The machine-readable reason Google gave, already sanitised, or `None` when it gave none.
#[must_use]
pub fn of(body: &[u8]) -> Option<String> {
    let envelope: ErrorEnvelope = serde_json::from_slice(body).ok()?;
    let error = envelope.error?;
    error
        .errors
        .into_iter()
        .find_map(|detail| detail.reason)
        .or(error.status)
        .map(|value| sanitize(&value))
}

/// A provider's reason code, reduced to something that is safe to put in a message.
#[must_use]
pub fn sanitize(value: &str) -> String {
    let trimmed = value.trim();
    let is_reason = !trimmed.is_empty()
        && trimmed.len() <= 40
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if is_reason {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{of, sanitize};

    #[test]
    fn a_reason_is_read_out_of_the_document_google_actually_sends() {
        assert_eq!(
            of(
                br#"{"error":{"code":403,"message":"The download quota for this file has been exceeded.","errors":[{"domain":"usageLimits","reason":"downloadQuotaExceeded","message":"..."}]}}"#
            )
            .as_deref(),
            Some("downloadQuotaExceeded")
        );
        // Newer endpoints send no `errors` array at all; `status` stands in.
        assert_eq!(
            of(br#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED"}}"#).as_deref(),
            Some("RESOURCE_EXHAUSTED")
        );
        assert_eq!(of(b"{}"), None);
        assert_eq!(of(b"<html>502</html>"), None);
    }

    /// Nothing a provider wrote travels verbatim, and a value that is not a reason code loses
    /// all of itself rather than being filtered down to its digits.
    #[test]
    fn a_providers_text_never_travels_verbatim() {
        assert_eq!(sanitize("downloadQuotaExceeded"), "downloadQuotaExceeded");
        assert_eq!(sanitize("RESOURCE_EXHAUSTED"), "RESOURCE_EXHAUSTED");
        assert_eq!(sanitize("token ya29.a0AfB_xyz rejected"), "refused");
        assert_eq!(sanitize("<html>500</html>"), "refused");
        assert_eq!(sanitize(""), "refused");
        assert_eq!(sanitize(&"x".repeat(200)), "refused");
    }
}

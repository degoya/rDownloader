//! What Graph said no with, reduced to something safe to repeat.
//!
//! Shared because the rule has to be the same everywhere it is applied, and because getting it
//! wrong is silent: a Graph error document carries a `message` written for a developer, and an
//! endpoint that echoed part of a token into one would publish it through a log line and
//! through the failure the interface shows.
//!
//! The rule is RD-105-01's, as RD-106-04 adapted it for an API rather than a token endpoint.
//! Graph's error codes are lowerCamelCase (`itemNotFound`, `accessDenied`,
//! `activityLimitReached`) with a few PascalCase ones from the sign-in layer
//! (`InvalidAuthenticationToken`), so the accepted shape is ASCII letters, digits and
//! underscores. What does *not* change is the important half: a value that is not that shape
//! is dropped **whole** rather than filtered character by character, because filtering keeps
//! the digits of a leaked token. `message` and `innerError` are never read at all: one is a
//! sentence, the other a bag of request ids, and neither has a shape check that makes it safe.

use serde::Deserialize;

/// The error document Graph returns for every refusal, at every endpoint.
#[derive(Debug, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub code: Option<String>,
}

/// The machine-readable code Graph gave, already sanitised, or `None` when it gave none.
#[must_use]
pub fn of(body: &[u8]) -> Option<String> {
    let envelope: ErrorEnvelope = serde_json::from_slice(body).ok()?;
    envelope.error?.code.map(|value| sanitize(&value))
}

/// A provider's error code, reduced to something that is safe to put in a message.
#[must_use]
pub fn sanitize(value: &str) -> String {
    let trimmed = value.trim();
    let is_code = !trimmed.is_empty()
        && trimmed.len() <= 40
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if is_code {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{of, sanitize};

    #[test]
    fn a_code_is_read_out_of_the_document_graph_actually_sends() {
        assert_eq!(
            of(
                br#"{"error":{"code":"itemNotFound","message":"The resource could not be found.",
                    "innerError":{"date":"2026-09-10T10:00:00","request-id":"r","client-request-id":"c"}}}"#
            )
            .as_deref(),
            Some("itemNotFound")
        );
        assert_eq!(
            of(br#"{"error":{"code":"InvalidAuthenticationToken","message":"Access token has expired."}}"#)
                .as_deref(),
            Some("InvalidAuthenticationToken")
        );
        assert_eq!(of(b"{}"), None);
        assert_eq!(of(b"<html>502</html>"), None);
    }

    /// Nothing a provider wrote travels verbatim, and a value that is not a code loses all of
    /// itself rather than being filtered down to its digits.
    #[test]
    fn a_providers_text_never_travels_verbatim() {
        assert_eq!(sanitize("accessDenied"), "accessDenied");
        assert_eq!(sanitize("activityLimitReached"), "activityLimitReached");
        assert_eq!(sanitize("token EwBwA8l6BAAU rejected"), "refused");
        assert_eq!(sanitize("<html>500</html>"), "refused");
        assert_eq!(sanitize(""), "refused");
        assert_eq!(sanitize(&"x".repeat(200)), "refused");
    }
}

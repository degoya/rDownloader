//! What Box said no with, reduced to something safe to repeat.
//!
//! Shared because the rule has to be the same everywhere it is applied, and because getting it
//! wrong is silent: a Box error document carries a `message` and a `context_info` written for a
//! developer, and an endpoint that echoed part of a token into one would publish it through a
//! log line and through the failure the interface shows.
//!
//! The rule is RD-105-01's, as RD-106-04 adapted it for an API rather than a token endpoint.
//! Box's error codes are lowercase words joined by underscores (`not_found`, `forbidden`,
//! `rate_limit_exceeded`, `access_denied_insufficient_permissions`), so the accepted shape is
//! ASCII lowercase letters, digits and underscores. What does *not* change is the important
//! half: a value that is not that shape is dropped **whole** rather than filtered character by
//! character, because filtering keeps the digits of a leaked token. `message`, `help_url`,
//! `request_id` and `context_info` are never read at all: the first is a sentence, and for a
//! sentence there is no shape check that makes it safe.

use serde::Deserialize;

/// The error document Box returns for every refusal, at every endpoint.
#[derive(Debug, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub code: Option<String>,
    /// The token endpoint answers in RFC 6749's vocabulary instead, under `error`.
    #[serde(default)]
    pub error: Option<String>,
}

/// The machine-readable code Box gave, already sanitised, or `None` when it gave none.
#[must_use]
pub fn of(body: &[u8]) -> Option<String> {
    let envelope: ErrorEnvelope = serde_json::from_slice(body).ok()?;
    envelope
        .code
        .or(envelope.error)
        .map(|value| sanitize(&value))
}

/// A provider's error code, reduced to something that is safe to put in a message.
#[must_use]
pub fn sanitize(value: &str) -> String {
    let trimmed = value.trim();
    let is_code = !trimmed.is_empty()
        && trimmed.len() <= 60
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
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
    fn a_code_is_read_out_of_the_document_box_actually_sends() {
        assert_eq!(
            of(br#"{"type":"error","status":404,"code":"not_found",
                    "context_info":{"errors":[{"reason":"invalid_parameter"}]},
                    "help_url":"http://developers.box.com/docs/#errors",
                    "message":"Not Found","request_id":"abcdef123456"}"#)
            .as_deref(),
            Some("not_found")
        );
        assert_eq!(
            of(br#"{"error":"invalid_grant","error_description":"Refresh token has expired"}"#)
                .as_deref(),
            Some("invalid_grant")
        );
        assert_eq!(of(b"{}"), None);
        assert_eq!(of(b"<html>502</html>"), None);
    }

    /// Nothing a provider wrote travels verbatim, and a value that is not a code loses all of
    /// itself rather than being filtered down to its digits.
    #[test]
    fn a_providers_text_never_travels_verbatim() {
        assert_eq!(sanitize("rate_limit_exceeded"), "rate_limit_exceeded");
        assert_eq!(
            sanitize("access_denied_insufficient_permissions"),
            "access_denied_insufficient_permissions"
        );
        assert_eq!(sanitize("token 1!vcS2tG rejected"), "refused");
        assert_eq!(sanitize("Not Found"), "refused");
        assert_eq!(sanitize("<html>500</html>"), "refused");
        assert_eq!(sanitize(""), "refused");
        assert_eq!(sanitize(&"x".repeat(200)), "refused");
    }
}

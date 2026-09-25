//! What Dropbox said no with, reduced to something safe to repeat.
//!
//! Shared because the rule has to be the same everywhere it is applied, and because getting it
//! wrong is silent: an error document carries an `error_summary` and a `user_message` written
//! for people, and an endpoint that echoed part of a token into one would publish it through
//! a log line and through the failure the interface shows.
//!
//! The rule is RD-105-01's and RD-106-04's: a value that is not the shape of a reason code is
//! dropped **whole** rather than filtered character by character, because filtering keeps the
//! digits of a leaked token. Dropbox's reasons are tagged unions — `{"error": {".tag": "path",
//! "path": {".tag": "not_found"}}}` — so the shape read here is the chain of tags joined by
//! `/`, `path/not_found`, which is also how Dropbox's own `error_summary` starts.

use serde_json::Value;

/// How many tags deep a reason is followed. Dropbox nests two; three is generous.
const MAX_DEPTH: usize = 3;

/// The machine-readable reason Dropbox gave, already sanitised, or `None` when it gave none.
#[must_use]
pub fn of(body: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let mut parts = Vec::new();
    if let Some(error) = document.get("error") {
        let mut node = error;
        for _ in 0..MAX_DEPTH {
            let Some(tag) = node.get(".tag").and_then(Value::as_str) else {
                break;
            };
            parts.push(tag.to_owned());
            match node.get(tag) {
                Some(inner) if inner.is_object() => node = inner,
                _ => break,
            }
        }
        // A rate limit has no top-level tag: `{"error": {"reason": {".tag": …}}}`.
        if parts.is_empty()
            && let Some(reason) = error
                .get("reason")
                .and_then(|reason| reason.get(".tag"))
                .and_then(Value::as_str)
        {
            parts.push(reason.to_owned());
        }
    }
    if parts.is_empty()
        && let Some(summary) = document.get("error_summary").and_then(Value::as_str)
    {
        parts.extend(
            summary
                .split('/')
                .filter(|part| !part.is_empty() && *part != "..")
                .map(str::to_owned),
        );
    }
    if parts.is_empty() {
        return None;
    }
    Some(sanitize(&parts.join("/")))
}

/// The `retry_after` a rate-limit document carries, in seconds, when it carries one.
#[must_use]
pub fn retry_after_in(body: &[u8]) -> Option<u64> {
    let document: Value = serde_json::from_slice(body).ok()?;
    document
        .get("error")
        .and_then(|error| error.get("retry_after"))
        .and_then(Value::as_u64)
}

/// A provider's reason, reduced to something that is safe to put in a message: up to three
/// tags of letters, digits and underscores, joined by `/`. Anything else is dropped whole.
#[must_use]
pub fn sanitize(value: &str) -> String {
    let trimmed = value.trim();
    let is_reason = !trimmed.is_empty()
        && trimmed.len() <= 80
        && trimmed.split('/').count() <= MAX_DEPTH
        && trimmed.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        });
    if is_reason {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{of, retry_after_in, sanitize};

    #[test]
    fn a_reason_is_read_out_of_the_documents_dropbox_actually_sends() {
        assert_eq!(
            of(
                br#"{"error_summary":"path/not_found/...","error":{".tag":"path","path":{".tag":"not_found"}}}"#
            )
            .as_deref(),
            Some("path/not_found")
        );
        assert_eq!(
            of(
                br#"{"error_summary":"expired_access_token/","error":{".tag":"expired_access_token"}}"#
            )
            .as_deref(),
            Some("expired_access_token")
        );
        assert_eq!(
            of(
                br#"{"error_summary":"shared_link_access_denied/..","error":{".tag":"shared_link_access_denied"}}"#
            )
            .as_deref(),
            Some("shared_link_access_denied")
        );
        // The rate-limit document has no top-level tag.
        let limited = br#"{"error_summary":"too_many_requests/..","error":{"reason":{".tag":"too_many_requests"},"retry_after":300}}"#;
        assert_eq!(of(limited).as_deref(), Some("too_many_requests"));
        assert_eq!(retry_after_in(limited), Some(300));
        // A summary alone still yields its tags.
        assert_eq!(
            of(br#"{"error_summary":"missing_scope/.."}"#).as_deref(),
            Some("missing_scope")
        );
        assert_eq!(of(b"{}"), None);
        assert_eq!(of(b"Error in call to API function"), None);
    }

    /// Nothing a provider wrote travels verbatim, and a value that is not a reason loses all of
    /// itself rather than being filtered down to its digits.
    #[test]
    fn a_providers_text_never_travels_verbatim() {
        assert_eq!(sanitize("path/not_found"), "path/not_found");
        assert_eq!(sanitize("too_many_requests"), "too_many_requests");
        assert_eq!(sanitize("token sl.u.AFx rejected"), "refused");
        assert_eq!(sanitize("<html>500</html>"), "refused");
        assert_eq!(sanitize(""), "refused");
        assert_eq!(sanitize("a//b"), "refused");
        assert_eq!(sanitize(&"x/".repeat(50)), "refused");
        // A `user_message` sentence quoted into the tag position is dropped whole too.
        assert_eq!(
            of(br#"{"error":{".tag":"The token sl.u.AFx has expired"}}"#).as_deref(),
            Some("refused")
        );
    }
}

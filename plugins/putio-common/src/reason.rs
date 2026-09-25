//! What may be repeated out of a Put.io error document, and what may not.
//!
//! Put.io answers every refusal with the same envelope:
//!
//! ```json
//! {"error_type": "NOT_FOUND", "error_message": "File not found.", "status_code": 404}
//! ```
//!
//! `error_type` is a stable word from a documented set, so it travels: it is what a message
//! catalogue can be keyed on and what a person can be told apart by. `error_message` is an
//! English sentence written for whoever is holding the API keys, and none of it survives —
//! there is no shape check that makes a sentence safe, and an endpoint that echoed part of a
//! token into its message would otherwise publish it into a log and into the interface.

use serde::Deserialize;

/// The envelope every Put.io endpoint answers a refusal with.
#[derive(Debug, Default, Deserialize)]
pub struct ErrorEnvelope {
    /// The stable word. Read, sanitised and forwarded as a parameter.
    #[serde(default)]
    pub error_type: Option<String>,
    /// Put.io's own sentence. Read only so its presence can be detected; never forwarded.
    #[serde(default)]
    pub error_message: Option<String>,
    /// The status Put.io states in the body, which is not always the one it sent.
    #[serde(default)]
    pub status_code: Option<u16>,
}

impl ErrorEnvelope {
    /// Reads an envelope out of a response body, answering an empty one for anything that is
    /// not JSON. A body that cannot be read is not evidence of success, so the caller decides
    /// from the status; what it must not do is fail because Put.io served an HTML error page.
    #[must_use]
    pub fn of(body: &[u8]) -> Self {
        serde_json::from_slice(body).unwrap_or_default()
    }

    /// Whether the document describes a refusal at all.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        self.error_type.is_some() || self.error_message.is_some()
    }

    /// The sanitised `error_type`, or `None` when there is nothing safe to repeat.
    #[must_use]
    pub fn kind(&self) -> Option<String> {
        self.error_type.as_deref().and_then(sanitize)
    }
}

/// The longest `error_type` that is repeated. Put.io's own are well under twenty characters;
/// anything longer is not one of theirs.
const MAX_LENGTH: usize = 40;

/// A Put.io `error_type`, reduced to something that is safe to put in a message.
///
/// Put.io spells its error types as upper-case words joined by underscores, so anything that
/// is not exactly that shape is dropped whole rather than filtered character by character.
/// Filtering would keep the digits of a leaked token; dropping keeps nothing.
#[must_use]
pub fn sanitize(error_type: &str) -> Option<String> {
    let trimmed = error_type.trim();
    let is_error_type = !trimmed.is_empty()
        && trimmed.len() <= MAX_LENGTH
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'_');
    is_error_type.then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{ErrorEnvelope, sanitize};

    #[test]
    fn the_stable_word_travels_and_the_sentence_does_not() {
        let envelope = ErrorEnvelope::of(
            br#"{"error_type":"NOT_FOUND","error_message":"File not found.","status_code":404}"#,
        );
        assert!(envelope.is_refusal());
        assert_eq!(envelope.kind().as_deref(), Some("NOT_FOUND"));
        assert_eq!(envelope.status_code, Some(404));
    }

    #[test]
    fn anything_that_is_not_an_error_type_is_dropped_whole() {
        assert_eq!(sanitize("ACCESS_DENIED").as_deref(), Some("ACCESS_DENIED"));
        // A sentence, a quoted credential and a served HTML page all keep nothing.
        assert_eq!(sanitize("File not found."), None);
        assert_eq!(sanitize("token ABC123XYZ rejected"), None);
        assert_eq!(sanitize("<html>500</html>"), None);
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize(&"X".repeat(200)), None);
    }

    #[test]
    fn an_answer_that_is_not_json_is_not_a_refusal_and_not_a_panic() {
        let envelope = ErrorEnvelope::of(b"<html><body>502 Bad Gateway</body></html>");
        assert!(!envelope.is_refusal());
        assert_eq!(envelope.kind(), None);
    }
}

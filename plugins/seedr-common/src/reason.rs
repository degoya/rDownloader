//! What may be repeated out of a Seedr refusal, and what may not.
//!
//! Seedr's REST v1 does not have one error envelope, and pretending it does is how a refusal
//! ends up read out of a perfectly good answer. Three shapes are in the field:
//!
//! - `{"result": false, "error": "<word or sentence>"}` on a call that refused;
//! - `{"result": "<word>", "code": 400}` — a refusal whose *result* is the reason, which is how
//!   a transfer that did not fit answers (`not_enough_space_added_to_wishlist`);
//! - the bare HTTP status, with a body that is an HTML page, which is what an expired or
//!   mistyped credential gets.
//!
//! Only a code-shaped word travels, and it travels as a parameter. A sentence never does:
//! there is no shape check that makes one safe, and this provider's credential is an e-mail
//! address and a password — an endpoint that echoed part of what was sent to it would publish
//! them into a log and into the interface.

use serde::Deserialize;

/// The refusal envelope, read out of whichever of the two JSON shapes arrived.
#[derive(Debug, Default, Deserialize)]
pub struct ErrorEnvelope {
    /// `true` on success; `false` or a reason word on a refusal.
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// The word or sentence Seedr put beside it.
    #[serde(default)]
    pub error: Option<String>,
    /// The status Seedr states in the body, which is not always the one it sent.
    #[serde(default)]
    pub code: Option<u16>,
}

impl ErrorEnvelope {
    /// Reads an envelope out of a response body.
    ///
    /// **Only a JSON object is read.** Serde builds a struct from a sequence as readily as
    /// from a map, taking the elements in field order, so a bare array read straight into this
    /// would become `{result: <first>, error: <second>}` — a refusal invented out of a good
    /// answer. A body that is not an object is simply not a refusal, and the caller decides
    /// from the status instead.
    #[must_use]
    pub fn of(body: &[u8]) -> Self {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .filter(serde_json::Value::is_object)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// Whether the document describes a refusal at all.
    ///
    /// `result` is the hard part: `true` is a success, `false` is a refusal, and **a string is
    /// also a refusal** — that is how Seedr answers a transfer it would not start. A missing
    /// `result` says nothing either way, which is what an endpoint answering a plain document
    /// does.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        match self.result.as_ref() {
            Some(serde_json::Value::Bool(ok)) => !ok,
            Some(serde_json::Value::String(word)) => !word.is_empty(),
            _ => self.error.is_some(),
        }
    }

    /// The sanitised reason, or `None` when there is nothing safe to repeat.
    ///
    /// `error` first, because a call that names one has named the reason; the string form of
    /// `result` is the fallback, because that is where a refused transfer puts it.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.error
            .as_deref()
            .and_then(sanitize)
            .or_else(|| match self.result.as_ref() {
                Some(serde_json::Value::String(word)) => sanitize(word),
                _ => None,
            })
    }
}

/// The longest reason that is repeated. Seedr's own are well under thirty characters; anything
/// longer is a sentence wearing a word's clothes.
const MAX_LENGTH: usize = 40;

/// A Seedr reason, reduced to something that is safe to put in a message.
///
/// Kept only when the *whole* of it is a short, code-shaped token, and dropped whole otherwise.
/// Filtering an answer that had echoed a credential would keep its characters; dropping keeps
/// nothing.
#[must_use]
pub fn sanitize(reason: &str) -> Option<String> {
    let trimmed = reason.trim();
    let code_shaped = !trimmed.is_empty()
        && trimmed.len() <= MAX_LENGTH
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    code_shaped.then(|| trimmed.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{ErrorEnvelope, sanitize};

    #[test]
    fn a_refusal_that_names_its_reason_keeps_the_word() {
        let envelope =
            ErrorEnvelope::of(br#"{"result":false,"error":"invalid_magnet","code":400}"#);
        assert!(envelope.is_refusal());
        assert_eq!(envelope.reason().as_deref(), Some("invalid_magnet"));
        assert_eq!(envelope.code, Some(400));
    }

    /// Seedr's second shape: the refusal *is* the result, which is how a transfer that did not
    /// fit in the account answers.
    #[test]
    fn a_reason_stated_as_the_result_is_still_a_refusal() {
        let envelope =
            ErrorEnvelope::of(br#"{"result":"not_enough_space_added_to_wishlist","code":400}"#);
        assert!(envelope.is_refusal());
        assert_eq!(
            envelope.reason().as_deref(),
            Some("not_enough_space_added_to_wishlist")
        );
    }

    #[test]
    fn a_success_is_not_a_refusal() {
        let envelope = ErrorEnvelope::of(br#"{"result":true,"code":200,"user_torrent_id":11}"#);
        assert!(!envelope.is_refusal());
        assert_eq!(envelope.reason(), None);
    }

    #[test]
    fn a_sentence_and_an_html_page_keep_nothing() {
        assert_eq!(sanitize("Too many requests, please slow down."), None);
        assert_eq!(sanitize("<html>500</html>"), None);
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize(&"x".repeat(200)), None);
        assert_eq!(sanitize("  Invalid_Magnet "), Some("invalid_magnet".into()));
    }

    #[test]
    fn an_answer_that_is_not_an_object_is_not_a_refusal() {
        for body in [
            &br#"["https://a","https://b"]"#[..],
            b"<html>502</html>",
            b"",
        ] {
            let envelope = ErrorEnvelope::of(body);
            assert!(!envelope.is_refusal());
            assert_eq!(envelope.reason(), None);
        }
    }
}

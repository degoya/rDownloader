//! What both TorBox API plugins have to answer identically (RD-1120-10).
//!
//! The resolver `plugins/torbox/` and the remote job `plugins/torbox-jobs/` are two plugins
//! because a manifest carries exactly one `plugin_type`, and both read the same envelope out of
//! the same API: `{"success": bool, "error": <word|null>, "detail": "<sentence>", "data": ...}`.
//! How that envelope is read, which identifiers are safe to send back, and in which order the
//! word, the `success` flag and the HTTP status are believed live here once. The buckets the
//! words fall into differ between the two (the job endpoints have words the resolver never
//! sees), so each plugin hands in its own classification and its own words as [`Words`].
//!
//! Nothing here makes a request and nothing here depends on a host, so the same code compiles
//! into the native resolver and into both WebAssembly components.

#![forbid(unsafe_code)]

use plugin_common::failure::{ApiFailure, ErrorKind, HttpWords, Message};
use serde::Deserialize;

/// The envelope every endpoint answers in, read for its failure half alone.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub success: Option<bool>,
    /// TorBox's stable upper-case word. Typed as a free value because the field is `null` on
    /// success, `false` at one or two endpoints, and a string when it means something.
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    // `detail` is deliberately not read: it is prose TorBox wrote, and nothing here forwards
    // a provider's sentence.
}

impl ErrorEnvelope {
    /// The error word, upper-cased, or `None` when the answer names none.
    #[must_use]
    pub fn code(&self) -> Option<String> {
        let text = self.error.as_ref()?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_ascii_uppercase())
    }
}

/// Whether an identifier TorBox handed out, or an address carried, is safe to put back into a
/// request.
///
/// It goes out again in a path or a query, so it is checked rather than trusted: an identifier
/// carrying a slash or an ampersand would be a request to somewhere else on the very host the
/// plugin is allowed to reach.
#[must_use]
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// How one TorBox plugin names a refusal.
pub struct Words {
    /// The status classes no document in the answer explains.
    pub http: HttpWords,
    /// The bucket a documented `error` word falls into, given the stated `Retry-After`.
    pub classify: fn(&str, Option<u64>) -> ApiFailure,
    /// The code `classify` answers a word it has no bucket for with.
    pub api_error: &'static str,
    /// A `success: false` that names no word and carries a 2xx.
    pub refused: Message,
}

/// The failure an answer describes, or `None` when it describes none.
///
/// An answer is a failure when it names an `error`, whatever its HTTP status; a 200 carrying
/// one is still a refusal, and a 4xx carrying none is classified by its status alone. Both
/// directions matter, because TorBox uses both.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
    words: &Words,
) -> Option<ApiFailure> {
    if let Some(api_code) = envelope.code() {
        let classified = (words.classify)(&api_code, retry_after);
        // A word this build has no bucket for is not the end of what the answer said. When the
        // status carries a meaning of its own -- a 429, a 5xx, a 401 -- that meaning is better
        // than "permanent, unknown word", and it is the difference between a wait and a job
        // somebody has to start again by hand. The word still travels as the parameter.
        if classified.code == words.api_error
            && let Err(by_status) = words.http.ensure_http_status(status, retry_after)
        {
            return Some(ApiFailure {
                params: vec![("api_code", api_code)],
                ..by_status
            });
        }
        return Some(classified);
    }
    if envelope.success == Some(false) {
        // A refusal TorBox did not name. The status is tried first, because most of these
        // carry one that says something; a `success: false` inside a 200 says only that the
        // call did not do what it was asked, and that is permanent rather than worth a retry.
        return Some(
            words
                .http
                .ensure_http_status(status, retry_after)
                .err()
                .unwrap_or_else(|| ApiFailure::new(ErrorKind::Permanent, words.refused)),
        );
    }
    if !(200..=299).contains(&status) {
        return words.http.ensure_http_status(status, retry_after).err();
    }
    None
}

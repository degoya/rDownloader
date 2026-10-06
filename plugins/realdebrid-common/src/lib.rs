//! What both Real-Debrid API plugins have to answer identically (RD-1120-10).
//!
//! The resolver `plugins/realdebrid/` and the remote job `plugins/realdebrid-torrents/` are two
//! plugins because a manifest carries exactly one `plugin_type`, and both read the same refusal
//! envelope out of the same API. Which of its two halves decides, and when the HTTP status does
//! instead, lives here once; the buckets a documented `error_code` falls into differ between
//! the two (the torrent endpoints add their own codes), so each plugin hands in its own
//! classification and its own words as [`Words`].
//!
//! Nothing here makes a request and nothing here depends on a host, so the same code compiles
//! into the native resolver and into both WebAssembly components.

#![forbid(unsafe_code)]

use plugin_common::failure::{ApiFailure, HttpWords};
use serde::Deserialize;

/// The failure envelope every endpoint answers a refusal with.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    /// The provider's own sentence. Read so its presence can be detected and **never**
    /// forwarded: a sentence Real-Debrid wrote can echo whatever was sent to it.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_code: Option<i64>,
}

/// How one Real-Debrid plugin names a refusal.
pub struct Words {
    /// The status classes no document in the answer explains.
    pub http: HttpWords,
    /// The bucket a documented `error_code` falls into, given the stated `Retry-After`.
    pub classify: fn(i64, Option<u64>) -> ApiFailure,
}

/// The failure an answer describes, or `None` when it describes none.
///
/// An answer is a failure when it carries an `error_code`, whatever its HTTP status; a 2xx
/// carrying one is still a refusal, and a 4xx carrying none is classified by its status alone.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
    words: &Words,
) -> Option<ApiFailure> {
    if let Some(api_code) = envelope.error_code {
        return Some((words.classify)(api_code, retry_after));
    }
    // A sentence with no number is still a refusal — it is just one the document does not
    // name, so it goes to the generic bucket by status rather than being read.
    if envelope.error.is_some() || !(200..=299).contains(&status) {
        return words.http.ensure_http_status(status, retry_after).err();
    }
    None
}

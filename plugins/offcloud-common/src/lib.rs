//! What both Offcloud plugins have to answer identically (RD-1120-10).
//!
//! Offcloud is two plugins -- the resolver `plugins/offcloud/` and the remote job
//! `plugins/offcloud-cloud/` -- because a manifest carries exactly one `plugin_type`. Both read
//! the same two refusal shapes out of the same API, `{"error": "<sentence>"}` and
//! `{"not_available": "<reason>"}`, and a copy of the rules in each would be two places for the
//! order between a word, a status and a sentence to drift. The rules live here once; the words
//! each plugin reports a refusal under are its own catalogue's, handed in as [`Words`].
//!
//! Nothing here makes a request and nothing here depends on a host, so the same code compiles
//! into the native resolver and into both WebAssembly components.

#![forbid(unsafe_code)]

use plugin_common::failure::{ApiFailure, ErrorKind, HttpWords, Message};
use serde::Deserialize;

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// multihoster plugins settled on.
pub const BUSY_SECONDS: u64 = 300;

/// How long an exhausted allowance is waited out.
///
/// Longer than the minute the other debrid plugins give a `429`, and on purpose: Offcloud
/// reports an exhausted allowance as a `429` and has no word of its own for it, so the status
/// stands for both, and an allowance does not refill within a minute. Asking again at once only
/// spends requests against the very budget that refused them.
pub const QUOTA_SECONDS: u64 = 3600;

/// The words one Offcloud plugin reports a refusal under: its own catalogue codes.
pub struct Words {
    /// The status classes no document in the answer explains.
    pub http: HttpWords,
    /// `NOAUTH`: the key is not a key any more.
    pub auth_invalid: Message,
    /// Any other word, and the prose that is dropped.
    pub api_error: Message,
    /// A `not_available` reason: the plan lacks an add-on.
    pub addon_required: Message,
}

/// The two refusal shapes, read out of one answer.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    /// The provider's own sentence. Read so its presence can be detected, and forwarded only
    /// when it is one of the stable words below.
    #[serde(default)]
    pub error: Option<String>,
    /// Which add-on the account would need for this link, when that is what stands in the way.
    #[serde(default, alias = "notAvailable")]
    pub not_available: Option<String>,
}

/// What is left of a provider's word once everything that is not code-shaped is gone.
///
/// Offcloud answers a refusal with a sentence, and a sentence can quote whatever was sent to
/// it — an address, and with the query-parameter entrance the key itself. So the value is kept
/// only when the whole of it is a short, code-shaped token, and dropped whole otherwise:
/// filtering an answer that echoed a credential would keep its digits.
#[must_use]
pub fn sanitize_error(reason: &str) -> Option<String> {
    let reason = reason.trim();
    let code_shaped = !reason.is_empty()
        && reason.len() <= 40
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    code_shaped.then(|| reason.to_ascii_lowercase())
}

/// Classifies the one stable word Offcloud puts in `error`, and buckets everything else.
///
/// `NOAUTH` is the only value the provider's own clients branch on, and it is the one that
/// matters: it says the key is not a key any more, which no amount of waiting repairs.
#[must_use]
pub fn classify_error(reason: &str, words: &Words) -> ApiFailure {
    match sanitize_error(reason).as_deref() {
        Some("noauth") => ApiFailure::new(ErrorKind::AccountInvalid, words.auth_invalid),
        Some(token) => ApiFailure::with_api_code(ErrorKind::Permanent, words.api_error, token),
        // Prose, and therefore nothing that can be shown or branched on. The category is
        // transient rather than permanent: an unreadable sentence is at least as likely to be
        // a passing outage as a verdict about this link.
        None => ApiFailure::new(ErrorKind::Transient(Some(BUSY_SECONDS)), words.api_error),
    }
}

/// Classifies the closed set of `not_available` reasons.
///
/// None of them is a fault of the link: each says the account's plan does not cover this kind
/// of download. `Unsupported` rather than `Permanent`, so the queue moves the link on to
/// another account or another way in instead of marking it dead.
#[must_use]
pub fn classify_not_available(reason: &str, words: &Words) -> ApiFailure {
    let token = sanitize_error(reason).unwrap_or_else(|| "unknown".to_owned());
    ApiFailure::new(ErrorKind::Unsupported, words.addon_required).with_param("addon", token)
}

/// The refusal an answer carries, or an empty envelope when it carries none.
///
/// **Only a JSON object can be a refusal**, and that has to be checked rather than assumed:
/// serde deserialises a struct from a *sequence* as readily as from a map, taking the elements
/// in field order. So `["https://a", "https://b"]` read straight into [`ErrorEnvelope`] becomes
/// `{error: "https://a", not_available: "https://b"}` -- a refusal invented out of a perfectly
/// good answer. Both `cloud/explore` and `cloud/history` answer with arrays, and before this
/// guard a finished job whose file tree came back in the bare-address shape was classified as
/// a missing add-on and arrived as an empty package.
#[must_use]
pub fn error_envelope(body: &[u8]) -> ErrorEnvelope {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .filter(serde_json::Value::is_object)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// The failure an answer describes, or `None` when it describes none.
///
/// Three sources disagree often enough that the order between them has to be written down
/// rather than fallen into:
///
/// 1. `not_available` wins outright. It is the one answer that names something a person can
///    do, and an answer carrying it and an error is still about the add-on.
/// 2. **A code-shaped `error` beats the status.** Offcloud answers a refusal with a 200 as
///    readily as with a 401, so a status-first rule would read `NOAUTH` on a 200 as no refusal
///    at all.
/// 3. **The status beats prose.** A 429 is a spent request budget whatever sentence rides
///    along with it, and reading that sentence instead would turn the one answer that carries
///    a `Retry-After` into a guess. This is the order the fixtures caught: before it,
///    `{"error": "Too many requests, please slow down."}` on a 429 was a five-minute wait
///    rather than the two minutes the header asked for.
/// 4. Prose on a 2xx is left: something refused this and nothing says what.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
    words: &Words,
) -> Option<ApiFailure> {
    if let Some(reason) = envelope.not_available.as_deref() {
        return Some(classify_not_available(reason, words));
    }
    if let Some(reason) = envelope
        .error
        .as_deref()
        .filter(|reason| sanitize_error(reason).is_some())
    {
        return Some(classify_error(reason, words));
    }
    if let Err(failure) = words.http.ensure_http_status(status, retry_after) {
        return Some(failure);
    }
    envelope
        .error
        .as_deref()
        .map(|reason| classify_error(reason, words))
}

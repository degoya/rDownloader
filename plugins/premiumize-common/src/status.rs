//! The envelope Premiumize wraps every answer in, and what its `code` means.
//!
//! Premiumize answers a refusal with HTTP `200` and `{"status":"error","code":"...",
//! "message":"..."}`. The status line is therefore not the answer — the body is — and a
//! reader that trusted the code would take `Not logged in` for a success. Measured against
//! all four `transfer/*` endpoints on 2026-09-22 and recorded in
//! `docs/roadmap/jobs/120-23-premiumize-nimmt-auftraege-entgegen.md`.
//!
//! `message` is the provider's own English sentence. It is read so that its *presence* can be
//! noticed and so an answer carrying no `code` can still be classified; it is never forwarded
//! to a log or an interface. What travels is `code`, which is stable and documented.

use serde::Deserialize;

/// The two fields every answer carries beside its own payload.
#[derive(Debug, Default, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    /// The provider's sentence. Classified, never quoted.
    #[serde(default)]
    pub message: Option<String>,
}

impl Envelope {
    /// Whether the service itself called this answer a success.
    ///
    /// An answer with no `status` at all is not one: every documented endpoint sends it, and a
    /// body without it is not this API's envelope.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status.as_deref() == Some("success")
    }
}

/// How a refusal is classified, without depending on any failure representation.
///
/// The same seven buckets `world remote-job-plugin` offers, named here so the classification
/// can be tested on the host target where the generated `failure-kind` does not exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AccountInvalid,
    RateLimited(Option<u64>),
    Unsupported,
}

/// How long a provider-side outage is waited out. Five minutes, the figure every other
/// multihoster plugin here settled on.
pub const BUSY_SECONDS: u64 = 300;

/// How long an exhausted quota is waited out.
pub const QUOTA_SECONDS: u64 = 3600;

/// Premiumize's own error vocabulary, grouped the way its documentation groups it.
///
/// The aliases are deliberate. `plugins/premiumize/src/resolver.rs` records that this plugin
/// family once matched six strings that appear nowhere in the published table, so every real
/// code fell through to permanent; the old strings are kept beside the documented ones
/// because an installation may still be answered by an older deployment and nothing here can
/// tell which.
#[must_use]
pub fn classify(code: Option<&str>, message: Option<&str>, retry_after: Option<u64>) -> Kind {
    match code.unwrap_or_default() {
        "link_generation_failed" | "transient_error" | "service_down" | "unknown_error" => {
            Kind::Transient(Some(BUSY_SECONDS))
        }
        "rate_limit_reached" => Kind::RateLimited(Some(retry_after.unwrap_or(60))),
        "service_limit_reached"
        | "account_limit_reached"
        | "semi_permanent_error"
        | "fairuse_limit"
        | "limit_exceeded" => Kind::RateLimited(Some(QUOTA_SECONDS)),
        "authentication_failed" | "not_logged_in" | "invalid_token" | "bad_token" => {
            Kind::AccountInvalid
        }
        "service_unsupported" | "unsupported" | "unsupported_service" => Kind::Unsupported,
        "not_found" => Kind::Offline,
        "permission_denied" | "invalid_request" | "permanent_error" => Kind::Permanent,
        // No code, or one this table has never seen. The envelope carries nothing else but
        // the sentence, so read that rather than calling every unknown answer permanent: the
        // source a multihoster will not take is the one case where the difference changes
        // what the interface may offer.
        _ => from_message(message),
    }
}

/// Last resort when no code was sent: Premiumize's messages are English and fixed phrases.
fn from_message(message: Option<&str>) -> Kind {
    let Some(message) = message else {
        return Kind::Permanent;
    };
    let message = message.to_ascii_lowercase();
    if message.contains("unsupported") || message.contains("not supported") {
        Kind::Unsupported
    } else if message.contains("not logged in") || message.contains("not authorized") {
        Kind::AccountInvalid
    } else {
        Kind::Permanent
    }
}

/// Maps an HTTP status for the cases where there is no envelope to read at all.
#[must_use]
pub fn from_http_status(status: u16, retry_after: Option<u64>) -> Option<Kind> {
    match status {
        200..=299 => None,
        401 | 403 => Some(Kind::AccountInvalid),
        404 | 410 => Some(Kind::Offline),
        429 => Some(Kind::RateLimited(Some(retry_after.unwrap_or(60)))),
        500..=599 => Some(Kind::Transient(Some(BUSY_SECONDS))),
        _ => Some(Kind::Permanent),
    }
}

/// Reads a `Retry-After` header stated in seconds. A date-shaped one is ignored rather than
/// guessed at: a wrong wait is worse than the bucket's own default.
#[must_use]
pub fn retry_after_seconds(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::{Envelope, Kind, QUOTA_SECONDS, classify, from_http_status, retry_after_seconds};

    fn envelope(body: &str) -> Envelope {
        serde_json::from_str(body).expect("an envelope")
    }

    /// The whole reason this module exists: the service reports its own failure with a 200.
    #[test]
    fn an_error_arrives_with_http_200_and_is_still_an_error() {
        let answer = envelope(
            r#"{"status":"error","message":"Not logged in","code":"authentication_failed"}"#,
        );
        assert!(!answer.is_success());
        assert_eq!(
            classify(answer.code.as_deref(), answer.message.as_deref(), None),
            Kind::AccountInvalid
        );
        assert_eq!(from_http_status(200, None), None, "the body is the answer");
    }

    #[test]
    fn a_success_is_a_success_and_a_body_without_a_status_is_not() {
        assert!(envelope(r#"{"status":"success","id":"x"}"#).is_success());
        assert!(!envelope(r#"{"id":"x"}"#).is_success());
    }

    #[test]
    fn each_documented_code_lands_in_its_own_bucket() {
        for (code, expected) in [
            ("service_down", Kind::Transient(Some(super::BUSY_SECONDS))),
            ("rate_limit_reached", Kind::RateLimited(Some(30))),
            ("fairuse_limit", Kind::RateLimited(Some(QUOTA_SECONDS))),
            ("authentication_failed", Kind::AccountInvalid),
            ("service_unsupported", Kind::Unsupported),
            ("not_found", Kind::Offline),
            ("permission_denied", Kind::Permanent),
        ] {
            assert_eq!(classify(Some(code), None, Some(30)), expected, "{code}");
        }
    }

    #[test]
    fn an_answer_with_no_code_is_read_from_its_sentence_and_never_quoted() {
        assert_eq!(
            classify(None, Some("This service is not supported"), None),
            Kind::Unsupported
        );
        assert_eq!(
            classify(None, Some("Not logged in"), None),
            Kind::AccountInvalid
        );
        assert_eq!(
            classify(None, Some("something else entirely"), None),
            Kind::Permanent
        );
        assert_eq!(classify(None, None, None), Kind::Permanent);
    }

    #[test]
    fn a_retry_after_in_seconds_is_read_and_a_date_shaped_one_is_not() {
        assert_eq!(retry_after_seconds(Some(" 120 ")), Some(120));
        assert_eq!(
            retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            None
        );
        assert_eq!(retry_after_seconds(None), None);
    }
}

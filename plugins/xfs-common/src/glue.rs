//! The glue between an XFS plugin's protocol logic and [`plugin_common`], once (RD-191-07,
//! PLUG-10).
//!
//! `ddownload`, `katfile`, `filejoker` and `xfs-generic` each carried the same dozen helpers in
//! their `resolver/api.rs` — the one-byte range probe, the HTML sniff, the `Set-Cookie` reader,
//! the status classification and the envelope conversion — different in nothing but the
//! plugin's translation codes. They live here now, taking those codes as arguments; a plugin's
//! `api.rs` keeps one-line wrappers that name its own codes, so its call sites read as before.
//! The HTML sniff and `coded` are every hoster's, so they are `plugin_common`'s and re-exported
//! here (RD-1110-03).

use plugin_common::failure::HttpError;
pub use plugin_common::failure::coded;
pub use plugin_common::is_html;
use plugin_common::retry_after;
use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse};
use serde::de::DeserializeOwned;

use crate::api::EnvelopeError;

/// A `GET` that asks for one byte, so a hotlink is recognised without downloading it.
#[must_use]
pub fn range_probe(url: impl Into<String>) -> HttpRequest {
    HttpRequest::get(url).with_header("Range", "bytes=0-0")
}

/// Every `Set-Cookie` value of a response, which `HttpResponse::header` cannot give: a sign-in
/// sets more than one and only one of them is the session.
#[must_use]
pub fn set_cookies(response: &HttpResponse) -> Vec<String> {
    response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.clone())
        .collect()
}

/// Classifies a transport status with the mapping every plugin shares
/// ([`plugin_common::http_status`]), carrying the response's `Retry-After` into a rate limit or
/// a server error. `code` and `text` are the plugin's `http_error` code and its English text.
/// A `404` or `410` is final, a `451` `Offline` (owner, 2026-10-04).
///
/// # Errors
///
/// The classified refusal, with a `status` parameter, for every status that is not a `2xx`.
pub fn ensure_http_status(
    response: &HttpResponse,
    code: &'static str,
    text: fn(u16) -> String,
) -> Result<(), Failure> {
    HttpError { code, text }
        .ensure_http_status(response.status, retry_after(&response.headers))
        .map_err(Failure::from)
}

/// The codes a plugin with an XFS JSON API reports its envelope failures under.
#[derive(Clone, Copy)]
pub struct ApiMessages {
    /// The `api_error` code; the failure carries the provider's `msg` as a `message` parameter.
    pub api_error: &'static str,
    /// The English text for an `api_error`.
    pub api_error_text: fn(&str) -> String,
    /// The `(code, message)` of an answer that is not the documented envelope.
    pub invalid_response: (&'static str, &'static str),
}

impl ApiMessages {
    /// An answer that is not the documented shape: worth one more try, never a verdict.
    #[must_use]
    pub fn invalid_response(&self) -> Failure {
        coded(FailureKind::Transient(None), self.invalid_response)
    }

    /// An [`EnvelopeError`] as a failure.
    #[must_use]
    pub fn envelope_error(&self, error: EnvelopeError) -> Failure {
        match error {
            EnvelopeError::Status(kind, message) => {
                Failure::coded(kind, self.api_error, (self.api_error_text)(&message))
                    .with_param("message", message)
            }
            EnvelopeError::MissingResult => self.invalid_response(),
        }
    }

    /// A JSON body, or [`Self::invalid_response`].
    ///
    /// # Errors
    ///
    /// The invalid-response failure when the body does not deserialise as `T`.
    pub fn parse_json<T: DeserializeOwned>(&self, response: &HttpResponse) -> Result<T, Failure> {
        serde_json::from_slice(&response.body).map_err(|_| self.invalid_response())
    }
}

#[cfg(test)]
mod tests {
    use plugin_common::{FailureKind, HttpResponse};

    use super::{ApiMessages, ensure_http_status, is_html, set_cookies};
    use crate::api::{EnvelopeError, ErrorKind};

    fn response(status: u16, headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status,
            final_url: "https://xfs.invalid/".to_owned(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn text(status: u16) -> String {
        format!("status {status}")
    }

    const API: ApiMessages = ApiMessages {
        api_error: "x.api_error",
        api_error_text: str::to_owned,
        invalid_response: ("x.invalid_response", "invalid"),
    };

    #[test]
    fn a_status_is_classified_with_the_shared_mapping() {
        assert!(ensure_http_status(&response(206, &[]), "x.http_error", text).is_ok());
        let gone =
            ensure_http_status(&response(404, &[]), "x.http_error", text).expect_err("a refusal");
        // A missing file is final (owner, 2026-10-04); a legal block is retried.
        assert_eq!(gone.kind, FailureKind::Permanent);
        assert_eq!(gone.code.as_deref(), Some("x.http_error"));
        assert_eq!(gone.params, vec![("status".to_owned(), "404".to_owned())]);
        let deleted =
            ensure_http_status(&response(410, &[]), "x.http_error", text).expect_err("a refusal");
        assert_eq!(deleted.kind, FailureKind::Permanent);
        let blocked =
            ensure_http_status(&response(451, &[]), "x.http_error", text).expect_err("a refusal");
        assert_eq!(blocked.kind, FailureKind::Offline);
        let refused =
            ensure_http_status(&response(403, &[]), "x.http_error", text).expect_err("a refusal");
        assert_eq!(refused.kind, FailureKind::AccountInvalid);
    }

    /// A rate limit carries the wait the provider named, which the copies dropped.
    #[test]
    fn a_rate_limit_carries_the_providers_wait() {
        let limited = ensure_http_status(
            &response(429, &[("Retry-After", "120")]),
            "x.http_error",
            text,
        )
        .expect_err("a refusal");
        assert_eq!(limited.kind, FailureKind::RateLimited(Some(120)));
        let busy =
            ensure_http_status(&response(503, &[]), "x.http_error", text).expect_err("a refusal");
        assert_eq!(busy.kind, FailureKind::Transient(None));
    }

    #[test]
    fn cookies_and_html_are_read_from_the_headers() {
        let page = response(
            200,
            &[
                ("Content-Type", "text/html; charset=utf-8"),
                ("Set-Cookie", "a=1"),
                ("set-cookie", "xfss=2"),
            ],
        );
        assert!(is_html(&page));
        assert_eq!(set_cookies(&page), vec!["a=1", "xfss=2"]);
        assert!(!is_html(&response(
            200,
            &[("Content-Type", "application/octet-stream")]
        )));
    }

    #[test]
    fn an_envelope_error_keeps_the_providers_message_as_a_parameter() {
        let failure = API.envelope_error(EnvelopeError::Status(
            ErrorKind::AccountInvalid,
            "Invalid key".to_owned(),
        ));
        assert_eq!(failure.kind, FailureKind::AccountInvalid);
        assert_eq!(failure.code.as_deref(), Some("x.api_error"));
        assert_eq!(
            failure.params,
            vec![("message".to_owned(), "Invalid key".to_owned())]
        );
        let missing = API.envelope_error(EnvelopeError::MissingResult);
        assert_eq!(missing.code.as_deref(), Some("x.invalid_response"));
        assert_eq!(missing.kind, FailureKind::Transient(None));
        assert!(API.parse_json::<u32>(&response(200, &[])).is_err());
    }
}

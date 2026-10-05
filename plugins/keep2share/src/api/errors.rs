//! Error classification, split out of `api.rs` to keep that file under the workspace's 500-line
//! limit (mirrors how `api/tests.rs` already carries the full IMPL-VERIFY provenance for this
//! same classification, for the same reason — see plugin-common.md). Re-exported wholesale via
//! `api.rs`'s `pub(crate) use errors::*;`, so every caller keeps addressing these as `api::X`.

pub(crate) use plugin_common::failure::{ApiFailure, ErrorKind};
use plugin_common::failure::{HttpError, HttpWords};
use serde::Deserialize;
use serde_json::Value;

use crate::messages;

/// The response body was not the JSON this API always answers with.
pub(crate) fn invalid_response() -> ApiFailure {
    ApiFailure::new(ErrorKind::Transient(None), messages::INVALID_RESPONSE)
}

/// A URL this plugin built or the API handed back failed to parse.
pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> ApiFailure {
    let text = messages::invalid_url(error);
    ApiFailure::new(ErrorKind::Permanent, (messages::INVALID_URL, text.as_str()))
        .with_param("error", error.to_string())
}

/// Minimal envelope probe every Keep2Share API endpoint answers with — enough of the shape to
/// detect and classify an error without committing to any one endpoint's success payload (parsed
/// separately by the caller once this probe reports no error). Mirrors JD's own
/// `entries.get("status")`/`entries.get("errorCode")`/`entries.get("code")`/`entries.get("message")`/
/// `entries.get("errors")` reads at the top of `handleErrorsAPI`.
#[derive(Deserialize)]
pub(crate) struct ErrorProbe {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default, rename = "errorCode")]
    pub(crate) error_code: Option<Value>,
    #[serde(default)]
    pub(crate) code: Option<i64>,
    #[serde(default)]
    pub(crate) message: Option<String>,
    #[serde(default)]
    pub(crate) errors: Option<Vec<SubError>>,
}

/// One entry of `ErrorProbe::errors` — JD unwraps this for a generic wrapper errorcode (21/22/42).
#[derive(Deserialize)]
pub(crate) struct SubError {
    #[serde(default)]
    pub(crate) code: Option<i64>,
    #[serde(default)]
    pub(crate) message: Option<String>,
    #[serde(default, rename = "timeRemaining")]
    pub(crate) time_remaining: Option<String>,
}

/// Classifies an [`ErrorProbe`]; `None` for success. Mirrors JD's `handleErrorsAPI` in its actual
/// precedence order — see `api/tests.rs`'s module doc for the full per-branch enumeration this
/// implements, with JD line references.
pub(crate) fn error_from_probe(probe: &ErrorProbe) -> Option<ApiFailure> {
    if let Some(Value::String(marker)) = &probe.error_code {
        if marker.eq_ignore_ascii_case("captcha_need_wait") {
            return Some(ApiFailure::new(
                ErrorKind::RateLimited(Some(60)),
                messages::FLOOD,
            ));
        }
        if marker.eq_ignore_ascii_case("captcha_need_wait_daily") {
            return Some(ApiFailure::new(
                ErrorKind::RateLimited(Some(1800)),
                messages::FLOOD,
            ));
        }
        // Unknown string enum: JD logs it and falls through to the numeric `code` field.
    }
    let numeric_error_code = match &probe.error_code {
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    };
    let Some(mut errorcode) = numeric_error_code.or(probe.code) else {
        return None; // No errorCode/code field at all -> success (JD: `return entries`).
    };
    if errorcode == 200
        && probe
            .status
            .as_deref()
            .is_some_and(|status| status.eq_ignore_ascii_case("success"))
    {
        return None;
    }
    let mut message = probe.message.clone().unwrap_or_default();
    let mut time_remaining = None;
    if matches!(errorcode, 21 | 22 | 42)
        && let Some(sub_error) = probe.errors.as_ref().and_then(|list| list.first())
    {
        if let Some(code) = sub_error.code {
            errorcode = code;
        }
        message = sub_error.message.clone().unwrap_or(message);
        time_remaining = sub_error.time_remaining.clone();
    }
    if message.eq_ignore_ascii_case("File not available") {
        return Some(ApiFailure::new(ErrorKind::Offline, messages::FILE_OFFLINE));
    }
    Some(classify_errorcode(
        errorcode,
        &message,
        time_remaining.as_deref(),
    ))
}

/// The `errorcode` switch itself — see [`error_from_probe`] and `api/tests.rs`'s module doc.
pub(crate) fn classify_errorcode(
    errorcode: i64,
    message: &str,
    time_remaining: Option<&str>,
) -> ApiFailure {
    match errorcode {
        1 => ApiFailure::new(
            ErrorKind::RateLimited(Some(3600)),
            messages::DOWNLOAD_LIMIT_REACHED,
        ),
        2 => ApiFailure::new(
            ErrorKind::RateLimited(Some(3600)),
            messages::TRAFFIC_EXHAUSTED,
        ),
        3 | 7 | 9 | 11 | 42 => ApiFailure::new(ErrorKind::AuthRequired, messages::PREMIUM_REQUIRED),
        4 => ApiFailure::new(ErrorKind::Permanent, messages::NO_ACCESS),
        5 => ApiFailure::new(
            ErrorKind::RateLimited(Some(download_wait_seconds(time_remaining))),
            messages::DOWNLOAD_WAIT,
        ),
        6 => ApiFailure::new(
            ErrorKind::RateLimited(Some(900)),
            messages::TOO_MANY_PARALLEL,
        ),
        8 => ApiFailure::new(ErrorKind::Permanent, messages::PRIVATE_FILE),
        10 | 75 => ApiFailure::new(ErrorKind::Transient(Some(60)), messages::SESSION_INVALID),
        20 | 23 => ApiFailure::new(ErrorKind::Offline, messages::FILE_OFFLINE),
        21 | 22 => ApiFailure::new(
            ErrorKind::Transient(None),
            messages::TEMPORARILY_UNAVAILABLE,
        ),
        30 | 33 => ApiFailure::new(ErrorKind::NeedsCaptcha, messages::LOGIN_CAPTCHA),
        // IMPL-VERIFY (`K2SApi.java:1540-1542`): errorcode 31 is `ERROR_CAPTCHA_INVALID` and
        // JD raises `LinkStatus.ERROR_CAPTCHA` for it, i.e. "the answer was wrong", not "a
        // captcha is required" (30/33). Split out of the 30/31/33 bucket so the free flow can
        // report `CaptchaFailed`; the premium flow never submits an answer and cannot reach it.
        31 => ApiFailure::new(ErrorKind::CaptchaFailed, messages::CAPTCHA_REJECTED),
        41 | 70 | 72 | 74 | 76 => {
            ApiFailure::new(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS)
        }
        71 => ApiFailure::new(ErrorKind::RateLimited(Some(1860)), messages::FLOOD),
        73 => ApiFailure::new(
            ErrorKind::RateLimited(Some(21_600)),
            messages::NETWORK_RESTRICTED,
        ),
        // 40 (wrong free-download key, unreachable in this plugin's premium-only flow — it never
        // sends `free_download_key`) and any other unrecognized errorcode: JD's own default
        // `switch` arm throws `PluginException(ERROR_PLUGIN_DEFECT, ...)` with no wait time —
        // non-retryable (K2SApi.java:1578-1582) — and the brief independently specifies
        // "unknown -> Permanent keep2share.api_error". Both agree; mapped `Permanent` here (an
        // earlier revision of this file mapped it `Transient{300}`, contradicting both JD and the
        // brief — a future/unmapped code would otherwise retry every 5 minutes forever instead of
        // surfacing as a failure).
        _ => {
            let text = messages::api_error(errorcode, message);
            ApiFailure::new(ErrorKind::Permanent, (messages::API_ERROR, text.as_str()))
                .with_param("api_status", errorcode.to_string())
                .with_param("message", message)
        }
    }
}

/// Errorcode 5's wait duration: JD reads `timeRemaining` (seconds, possibly with a fractional
/// part) from the unwrapped sub-error, truncating at the decimal point; falls back to 15 minutes
/// when absent or unparseable.
fn download_wait_seconds(time_remaining: Option<&str>) -> u64 {
    time_remaining
        .and_then(|value| value.split('.').next())
        .and_then(|whole| whole.parse::<u64>().ok())
        .unwrap_or(900)
}

/// Maps a bare HTTP status whose body did not parse as an [`ErrorProbe`] at all.
///
/// The classes are `plugin_common::http_status`'s, the one mapping every plugin shares
/// (RD-191-07): 401/403 -> `AccountInvalid`, 404/410 -> `Permanent`, 451 -> `Offline`, 429 ->
/// `RateLimited` with the response's `Retry-After`, 5xx -> `Transient`. Checked before it, because Keep2Share
/// documents it: JD's own `checkResponseCodeErrors` 400 case, a 5-minute retry ("This may
/// happen after any request even if the request itself is done right").
pub(crate) fn ensure_http_status(status: u16, retry_after: Option<u64>) -> Result<(), ApiFailure> {
    if status == 400 {
        return Err(ApiFailure::new(
            ErrorKind::Transient(Some(300)),
            messages::SERVER_ERROR,
        ));
    }
    HTTP.ensure_http_status(status, retry_after)
}

/// How Keep2Share's codes name each class of the shared mapping.
const HTTP: HttpWords = HttpWords {
    unauthorized: messages::BAD_CREDENTIALS,
    gone: messages::FILE_OFFLINE,
    unavailable: messages::FILE_OFFLINE,
    rate_limited: messages::FLOOD,
    server_error: messages::SERVER_ERROR,
    rate_limited_wait: None,
    server_error_wait: None,
    other: HttpError {
        code: messages::HTTP_ERROR,
        text: messages::http_error,
    },
};

#[cfg(test)]
#[path = "errors/tests.rs"]
mod tests;

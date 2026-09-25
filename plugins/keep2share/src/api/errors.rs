//! Error classification, split out of `api.rs` to keep that file under the workspace's 500-line
//! limit (mirrors how `api/tests.rs` already carries the full IMPL-VERIFY provenance for this
//! same classification, for the same reason — see plugin-common.md). Re-exported wholesale via
//! `api.rs`'s `pub(crate) use errors::*;`, so every caller keeps addressing these as `api::X`.

use serde::Deserialize;
use serde_json::Value;

use crate::messages;

/// Failure classification independent of the native (`rd_core::Failure`) and WASM
/// (WIT-generated `Failure`) representations; both adapters convert this into their own type.
#[derive(Debug)]
pub(crate) struct ApiFailure {
    pub(crate) kind: ErrorKind,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) params: Vec<(&'static str, String)>,
}

/// Mirrors `rd_core::FailureKind` / the WIT `failure-kind` variant, without depending on either.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    NeedsCaptcha,
    #[allow(dead_code)] // `matches()` filters unsupported links before any call is made.
    Unsupported,
    /// This IP may not start another free download yet — only the account-less flow reaches
    /// this (see [`crate::api::free`]); the scheduler blocks the hoster rather than the link.
    IpBlocked(Option<u64>),
    /// A captcha answer was submitted and rejected (errorcode 31).
    CaptchaFailed,
}

pub(crate) fn coded(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// The response body was not the JSON this API always answers with.
pub(crate) fn invalid_response() -> ApiFailure {
    coded(ErrorKind::Transient(None), messages::INVALID_RESPONSE)
}

/// A URL this plugin built or the API handed back failed to parse.
pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> ApiFailure {
    ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(error),
        params: vec![("error", error.to_string())],
    }
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
            return Some(coded(ErrorKind::RateLimited(Some(60)), messages::FLOOD));
        }
        if marker.eq_ignore_ascii_case("captcha_need_wait_daily") {
            return Some(coded(ErrorKind::RateLimited(Some(1800)), messages::FLOOD));
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
        return Some(coded(ErrorKind::Offline, messages::FILE_OFFLINE));
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
        1 => coded(
            ErrorKind::RateLimited(Some(3600)),
            messages::DOWNLOAD_LIMIT_REACHED,
        ),
        2 => coded(
            ErrorKind::RateLimited(Some(3600)),
            messages::TRAFFIC_EXHAUSTED,
        ),
        3 | 7 | 9 | 11 | 42 => coded(ErrorKind::AuthRequired, messages::PREMIUM_REQUIRED),
        4 => coded(ErrorKind::Permanent, messages::NO_ACCESS),
        5 => coded(
            ErrorKind::RateLimited(Some(download_wait_seconds(time_remaining))),
            messages::DOWNLOAD_WAIT,
        ),
        6 => coded(
            ErrorKind::RateLimited(Some(900)),
            messages::TOO_MANY_PARALLEL,
        ),
        8 => coded(ErrorKind::Permanent, messages::PRIVATE_FILE),
        10 | 75 => coded(ErrorKind::Transient(Some(60)), messages::SESSION_INVALID),
        20 | 23 => coded(ErrorKind::Offline, messages::FILE_OFFLINE),
        21 | 22 => coded(
            ErrorKind::Transient(None),
            messages::TEMPORARILY_UNAVAILABLE,
        ),
        30 | 33 => coded(ErrorKind::NeedsCaptcha, messages::LOGIN_CAPTCHA),
        // IMPL-VERIFY (`K2SApi.java:1540-1542`): errorcode 31 is `ERROR_CAPTCHA_INVALID` and
        // JD raises `LinkStatus.ERROR_CAPTCHA` for it, i.e. "the answer was wrong", not "a
        // captcha is required" (30/33). Split out of the 30/31/33 bucket so the free flow can
        // report `CaptchaFailed`; the premium flow never submits an answer and cannot reach it.
        31 => coded(ErrorKind::CaptchaFailed, messages::CAPTCHA_REJECTED),
        41 | 70 | 72 | 74 | 76 => coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS),
        71 => coded(ErrorKind::RateLimited(Some(1860)), messages::FLOOD),
        73 => coded(
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
        _ => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR,
            message: messages::api_error(errorcode, message),
            params: vec![
                ("api_status", errorcode.to_string()),
                ("message", message.to_owned()),
            ],
        },
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

/// Maps a bare HTTP status whose body did not parse as an [`ErrorProbe`] at all. Plugin-common's
/// stated HTTP conventions (401/403 -> `AccountInvalid`, 404/410/451 -> `Offline`, 429 ->
/// `RateLimited`, 5xx -> `Transient`), plus JD's own `checkResponseCodeErrors` 400 case (a
/// 5-minute retry — JD: "This may happen after any request even if the request itself is done
/// right").
pub(crate) fn ensure_http_status(status: u16) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        400 => Err(coded(
            ErrorKind::Transient(Some(300)),
            messages::SERVER_ERROR,
        )),
        401 | 403 => Err(coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS)),
        404 | 410 | 451 => Err(coded(ErrorKind::Offline, messages::FILE_OFFLINE)),
        429 => Err(coded(ErrorKind::RateLimited(None), messages::FLOOD)),
        500..=599 => Err(coded(ErrorKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
    }
}

#[cfg(test)]
#[path = "errors/tests.rs"]
mod tests;

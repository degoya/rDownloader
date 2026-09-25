//! Target-independent Rapidgator API v2 logic: URL matching, request/response shapes and error
//! classification. Shared verbatim by the native (`native.rs`) and WebAssembly (`guest.rs`)
//! adapters so both report byte-identical failure codes and messages; neither `rd-core` nor
//! `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`, which are
//! available on every target.
//!
//! IMPL-VERIFY summary (against JD's `RapidGatorNet.java`, the living reference —
//! `svn_trunk/src/jd/plugins/hoster/RapidGatorNet.java`, revision 53165): endpoints/params
//! (`user/login?login=&password=`, `file/info?token=&file_id=`, `file/download?token=&file_id=`)
//! and the `{"response": T|null, "status": <int>, "details": "..."}` envelope shape are confirmed
//! verbatim. `check_account` skips a separate `user/info` call — JD only uses that endpoint to
//! validate a *cached* session, which this plugin never has (`user/login`'s own response already
//! carries `user`/`storage`). [`classify_status`]/[`ensure_http_status`]'s doc comments carry the
//! full per-branch enumeration and retry-delay provenance; the exhaustive walk-through of every
//! `handleErrors_api` arm this implements lives in `api/tests.rs`'s module doc, next to the tests
//! that assert each one.

use serde::Deserialize;
use url::Url;

use crate::messages;

/// Bare hostnames (no `www.` prefix) this hoster's file links carry, per JD's
/// `getPluginDomains()`.
pub(crate) const MATCH_HOSTS: &[&str] = &["rapidgator.net", "rg.to", "rapidgator.asia"];

pub(crate) const API_BASE: &str = "https://rapidgator.net/api/v2";

/// Extracts the file id from a Rapidgator link, e.g. `https://rapidgator.net/file/<id>` or
/// `https://rapidgator.net/file/<id>/name.html`.
///
/// Mirrors JD's `getFID`: `(?i)/file/([a-z0-9]{32}|\d+)` — the host (`www.`-stripped) must be one
/// of [`MATCH_HOSTS`], the path must start with `/file/`, and the id is the first path segment
/// after it, accepted only if it is exactly 32 ASCII alphanumeric characters or one-or-more ASCII
/// digits (JD's `[a-z0-9]{32}` is case-insensitive, so any letter, not just hex digits).
pub(crate) fn file_id(url: &Url) -> Option<&str> {
    let host_str = url.host_str()?;
    let host = host_str.strip_prefix("www.").unwrap_or(host_str);
    if !MATCH_HOSTS.contains(&host) {
        return None;
    }
    let rest = url.path().strip_prefix("/file/")?;
    let segment = rest.split('/').next().unwrap_or("");
    let is_32_char_id = segment.len() == 32 && segment.chars().all(|c| c.is_ascii_alphanumeric());
    let is_numeric_id = !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit());
    (is_32_char_id || is_numeric_id).then_some(segment)
}

/// Validates the raw download URL string `file/download`'s `response.download_url` carries.
/// Shared so a malformed URL from the API produces the exact same `rapidgator.invalid_url`
/// failure on both the native and guest adapters instead of one erroring and the other silently
/// forwarding a URL that later fails to parse deeper in the pipeline (or not at all, on the guest
/// side, where `ResolvedDownload.url` is a bare `String`).
pub(crate) fn parse_download_url(raw: &str) -> Result<Url, ApiFailure> {
    Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// Generic `{"response": T|null, "status": <int>, "details": "..."}` envelope every Rapidgator
/// v2 endpoint answers with (see the module-level IMPL-VERIFY note on classification).
#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    pub(crate) response: Option<T>,
    pub(crate) status: Option<i64>,
    pub(crate) details: Option<String>,
}

/// `response` of `GET user/login`.
#[derive(Deserialize)]
pub(crate) struct LoginResult {
    pub(crate) token: Option<String>,
    pub(crate) user: Option<UserInfo>,
}

#[derive(Deserialize)]
pub(crate) struct UserInfo {
    pub(crate) is_premium: Option<bool>,
    pub(crate) premium_end_time: Option<i64>,
    #[serde(default)]
    pub(crate) traffic: Option<Traffic>,
}

#[derive(Deserialize)]
pub(crate) struct Traffic {
    pub(crate) left: Option<i64>,
}

/// `response` of `GET file/info`.
#[derive(Deserialize)]
pub(crate) struct FileInfoResult {
    pub(crate) file: Option<FileEntry>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct FileEntry {
    pub(crate) name: Option<String>,
    pub(crate) size: Option<i64>,
}

/// `response` of `GET file/download`.
#[derive(Deserialize)]
pub(crate) struct DownloadResult {
    pub(crate) download_url: Option<String>,
}

/// The subscription end the account label states, from `user/login`'s `response.user`
/// fields: the civil date of `premium_end_time`, and only while the account is premium.
pub(crate) fn premium_until(premium: bool, premium_end_time: Option<i64>) -> Option<String> {
    premium_end_time.filter(|_| premium).map(civil_date)
}

/// Formats a Unix timestamp (seconds) as a UTC `YYYY-MM-DD` date, using Howard Hinnant's
/// `civil_from_days` algorithm (pure integer arithmetic — `chrono` is a native-only dependency in
/// this workspace's plugin convention, and this module must stay usable from the WASM guest too).
fn civil_date(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// <http://howardhinnant.github.io/date_algorithms.html#civil_from_days>; `z` is a day count
/// relative to the Unix epoch (1970-01-01 = day 0).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

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
    #[allow(dead_code)] // Rapidgator's JSON API never challenges with a captcha.
    NeedsCaptcha,
    #[allow(dead_code)] // `matches()` filters unsupported links before any call is made.
    Unsupported,
}

fn coded(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// Classifies a non-success `status`/`details` pair from the envelope. Mirrors JD's
/// `handleErrors_api` in its actual precedence order (see `api/tests.rs`'s module doc for the
/// full per-branch enumeration this implements, with JD line references and retry-delay
/// provenance).
///
/// `trust_404` mirrors JD's own `trustError404` parameter to `handleErrors_api`: `file/info`
/// passes `true` (a 404 there is trusted unconditionally as "file offline"); `file/download`
/// passes `false`, because JD explicitly does *not* trust a 404 on that endpoint — its own
/// comment: "Rapidgator API IN SOME SITUATIONS has a bug which will return invalid offline
/// status. Do NOT trust this status anymore!" (this is an API-bug workaround, unrelated to
/// session caching). An untrusted 404 is retried instead of declared offline.
fn classify_status(status: i64, message: &str, trust_404: bool) -> ApiFailure {
    let lower = message.to_ascii_lowercase();
    if status == 423 || lower.contains("exceeded traffic") {
        return coded(ErrorKind::RateLimited(Some(300)), messages::LIMIT_REACHED);
    }
    if message.contains("Denied by IP") {
        return coded(ErrorKind::RateLimited(Some(7200)), messages::IP_DENIED);
    }
    if message.contains("Please wait") {
        return coded(ErrorKind::RateLimited(Some(300)), messages::LOGIN_THROTTLED);
    }
    if message.contains("User is not PREMIUM")
        || message.contains("This file can be downloaded by premium only")
        || message.contains("You can download files up to")
    {
        return coded(ErrorKind::AuthRequired, messages::PREMIUM_REQUIRED);
    }
    if message.contains("Login or password is wrong")
        || message.contains("Error: Error e-mail or password")
        || message.contains("Password cannot be blank")
        || message.contains("User is FROZEN")
        || lower
            .contains("error: account locked for violation of our terms. please contact support.")
        || message.contains("Parameter login or password is missing")
        || (status == 401 && lower.contains("wrong e-mail or password"))
    {
        return coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS);
    }
    if status == 401
        || lower.contains("session not exist")
        || lower.contains("session doesn't exist")
    {
        return coded(ErrorKind::Transient(None), messages::SESSION_INVALID);
    }
    match status {
        404 if trust_404 => coded(ErrorKind::Offline, messages::FILE_OFFLINE),
        404 => coded(
            ErrorKind::Transient(Some(60)),
            messages::DOWNLOAD_404_UNTRUSTED,
        ),
        500 => coded(ErrorKind::Transient(Some(300)), messages::SERVER_ERROR),
        503 => coded(ErrorKind::Transient(Some(1800)), messages::SERVER_ERROR),
        _ if lower.contains("this download session is not for you")
            || lower.contains("session not found") =>
        {
            coded(ErrorKind::Transient(None), messages::SESSION_INVALID)
        }
        // JD: `AccountUnavailableException(msg, 60_000)` — the same temporary-block exception
        // class used for "Denied by IP"/traffic-limit above, so `RateLimited` rather than
        // `AccountInvalid` (see `messages::IP_CONFIRMATION_REQUIRED`'s doc comment).
        _ if message
            .contains("Error: You requested login to your account from unusual Ip address") =>
        {
            coded(
                ErrorKind::RateLimited(Some(60)),
                messages::IP_CONFIRMATION_REQUIRED,
            )
        }
        _ => ApiFailure {
            // JD's own fallback for an unrecognized error is retryable (60s), not permanent.
            kind: ErrorKind::Transient(Some(60)),
            code: messages::API_ERROR,
            message: messages::api_error(status, message),
            params: vec![
                ("api_status", status.to_string()),
                ("message", message.to_owned()),
            ],
        },
    }
}

/// Checks an envelope's `status`/`details` pair; `None` for success (`status` absent or `200`).
/// `trust_404` — see [`classify_status`].
pub(crate) fn error_from_envelope(
    status: Option<i64>,
    details: Option<&str>,
    trust_404: bool,
) -> Option<ApiFailure> {
    let status = match status {
        None | Some(200) => return None,
        Some(value) => value,
    };
    Some(classify_status(
        status,
        details.unwrap_or("Unknown Rapidgator API error"),
        trust_404,
    ))
}

/// Maps a bare HTTP status whose body did not parse as an [`Envelope`] (JD's upfront
/// `con.getResponseCode()` checks, run before any JSON parsing is attempted). `trust_404` — see
/// [`classify_status`]; JD's bare-status check also calls `handle404API(..., trustError404)`, so
/// the same distinction applies here too.
pub(crate) fn ensure_http_status(status: u16, trust_404: bool) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 => Err(coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS)),
        404 if trust_404 => Err(coded(ErrorKind::Offline, messages::FILE_OFFLINE)),
        404 => Err(coded(
            ErrorKind::Transient(Some(60)),
            messages::DOWNLOAD_404_UNTRUSTED,
        )),
        416 => Err(coded(
            ErrorKind::Transient(Some(300)),
            messages::SERVER_ERROR,
        )),
        423 => Err(coded(
            ErrorKind::RateLimited(Some(300)),
            messages::LIMIT_REACHED,
        )),
        429 => Err(coded(ErrorKind::RateLimited(None), messages::RATE_LIMITED)),
        500 => Err(coded(
            ErrorKind::Transient(Some(3600)),
            messages::SERVER_ERROR,
        )),
        503 => Err(coded(
            ErrorKind::Transient(Some(300)),
            messages::SERVER_ERROR,
        )),
        501 | 502 | 504..=599 => Err(coded(ErrorKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
    }
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

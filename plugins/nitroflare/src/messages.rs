//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "nitroflare.account_missing",
    "Nitroflare account is missing",
);

/// The account has no Nitroflare premium key configured.
pub(crate) const PREMIUM_KEY_MISSING: (&str, &str) = (
    "nitroflare.premium_key_missing",
    "Nitroflare premium key is missing",
);

/// The URL is not a Nitroflare file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "nitroflare.unsupported_link",
    "Not a supported Nitroflare link",
);

/// API responded with error code `8` ("invalid login data"), or HTTP 401/403.
pub(crate) const BAD_CREDENTIALS: (&str, &str) = (
    "nitroflare.bad_credentials",
    "Nitroflare premium key is invalid or login failed",
);

/// API responded with error code `4` ("file doesn't exist"), or HTTP 404/410/451.
pub(crate) const FILE_OFFLINE: (&str, &str) = (
    "nitroflare.file_offline",
    "Nitroflare file is not available",
);

/// API responded with error code `1` ("access denied"): the file is premium-only or the account
/// is not premium.
pub(crate) const PREMIUM_REQUIRED: (&str, &str) = (
    "nitroflare.premium_required",
    "Nitroflare premium account is required to use the download API",
);

/// The provider's error message indicates the daily traffic/bandwidth allowance was exhausted;
/// safe to retry after the fixed one-hour cooldown the task brief specifies.
pub(crate) const TRAFFIC_EXHAUSTED: (&str, &str) = (
    "nitroflare.traffic_exhausted",
    "Nitroflare traffic limit was exceeded",
);

/// API responded with error code `12`: the API itself now demands a captcha to continue, which
/// this plugin cannot solve.
pub(crate) const CAPTCHA_REQUIRED: (&str, &str) = (
    "nitroflare.captcha_required",
    "Nitroflare requires solving a captcha to continue using the API",
);

/// API responded with error code `6` ("invalid captcha"). JD only reaches this after submitting
/// a captcha solution, which this plugin never does; kept distinct from `CAPTCHA_REQUIRED`
/// because the provider's own wording distinguishes "required" from "invalid".
pub(crate) const CAPTCHA_INVALID: (&str, &str) = (
    "nitroflare.captcha_invalid",
    "Nitroflare rejected a captcha response; this plugin cannot solve captchas",
);

/// HTTP 5xx or a network-level failure.
pub(crate) const SERVER_ERROR: (&str, &str) =
    ("nitroflare.server_error", "Nitroflare server error");

/// Bare HTTP 429 the JSON envelope didn't otherwise explain.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "nitroflare.rate_limited",
    "Nitroflare API rate limit was triggered",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("nitroflare.invalid_response", "Invalid Nitroflare response");

/// `getDownloadLink` reported no error but omitted the `result.url` field.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "nitroflare.no_download_url",
    "Nitroflare did not return a download URL",
);

// --- account-less (free) website flow ---------------------------------------------------------

/// The file page carried no reCAPTCHA site key, so the free flow has nothing to solve; carries a
/// `diagnosis`. An absent countdown is *not* an error — JD falls back to 60 seconds
/// (`crate::page::DEFAULT_WAIT_SECONDS`).
pub(crate) const NO_FREE_MARKERS: &str = "nitroflare.no_free_markers";

/// The free flow reached its last step but the answer carried no download link; carries a
/// `diagnosis`.
pub(crate) const NO_FREE_LINK: &str = "nitroflare.no_free_link";

/// The answer carried a download link on a host that does not belong to this hoster — refused
/// rather than followed. Carries the rejected `host`.
pub(crate) const FREE_LINK_HOST_MISMATCH: &str = "nitroflare.free_link_host_mismatch";

/// This IP may not start another free download yet; carries `wait_seconds` when the page stated
/// one. Mirrors the `ERROR_IP_BLOCKED` branches of JD's `handleErrors`.
pub(crate) const FREE_LIMIT_REACHED: &str = "nitroflare.free_limit_reached";

/// `POST /ajax/freeDownload.php method=startTimer` did not answer `1`; carries the `answer`. JD
/// treats this as a plugin defect after re-checking for a known error first.
pub(crate) const TIMER_NOT_STARTED: &str = "nitroflare.timer_not_started";

/// The hoster rejected the captcha answer ("The captcha wasn't entered correctly" / "You have to
/// fill the captcha").
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "nitroflare.captcha_rejected",
    "Nitroflare rejected the captcha answer",
);

pub(crate) fn no_free_markers(diagnosis: &str) -> String {
    format!("Nitroflare free download page was not recognized: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("Nitroflare free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_link_host_mismatch(host: &str) -> String {
    format!("Nitroflare returned a download link on an unexpected host: {host}")
}

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!(
            "Nitroflare free download limit reached; another download is possible in {seconds}s"
        ),
        None => "Nitroflare free download limit reached for this IP address".to_owned(),
    }
}

pub(crate) fn timer_not_started(answer: &str) -> String {
    format!("Nitroflare did not start the pre-download countdown (answer: {answer})")
}

/// The API envelope reported an error not covered by a specific code above; carries the
/// provider `api_code`/`message`.
pub(crate) const API_ERROR: &str = "nitroflare.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "nitroflare.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "nitroflare.invalid_url";

pub(crate) fn api_error(code: i64, message: &str) -> String {
    format!("Nitroflare API ({code}): {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("Nitroflare HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "rapidgator.account_missing",
    "Rapidgator account is missing",
);

/// The account has no Rapidgator password configured.
pub(crate) const PASSWORD_MISSING: (&str, &str) = (
    "rapidgator.password_missing",
    "Rapidgator password is missing",
);

/// The URL is not a Rapidgator file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "rapidgator.unsupported_link",
    "Not a supported Rapidgator link",
);

/// `user/login` reported "Login or password is wrong" (and the phrase variants JD also matches:
/// "Error: Error e-mail or password", "Password cannot be blank", "User is FROZEN", "Error:
/// ACCOUNT LOCKED FOR VIOLATION OF OUR TERMS...", "Parameter login or password is missing", or an
/// envelope `status` of 401 carrying "Wrong e-mail or password"), or a bare HTTP 401.
pub(crate) const BAD_CREDENTIALS: (&str, &str) = (
    "rapidgator.bad_credentials",
    "Rapidgator login or password is wrong",
);

/// `file/info` reported `status` 404, or a bare HTTP 404 on a call where a 404 is trusted. JD
/// trusts a `file/info` 404 unconditionally as offline (`requestFileInformationAPI` passes
/// `trustError404=true`). See [`DOWNLOAD_404_UNTRUSTED`] for why `file/download`'s 404 is *not*
/// classified this way.
pub(crate) const FILE_OFFLINE: (&str, &str) = (
    "rapidgator.file_offline",
    "Rapidgator file is not available",
);

/// `file/download` reported `status` 404, or a bare HTTP 404 on that same call. Unlike
/// `file/info`, JD does **not** trust this signal here (`handlePremium_api` passes
/// `trustError404=false`) — its own documented reason is an API bug, independent of session
/// caching: "Rapidgator API IN SOME SITUATIONS has a bug which will return invalid offline
/// status. Do NOT trust this status anymore!" JD retries instead of declaring the file offline
/// (`handleInvalidSession` → `throwAccountUnavailableException`, a 60-second wait); mirrored here
/// as a `Transient` retry rather than `Offline`.
pub(crate) const DOWNLOAD_404_UNTRUSTED: (&str, &str) = (
    "rapidgator.download_link_unconfirmed",
    "Rapidgator did not confirm the file is offline; retrying",
);

/// Envelope `status` 423 or a message mentioning "Exceeded traffic" (daily download/traffic
/// limit reached), or a bare HTTP 423.
pub(crate) const LIMIT_REACHED: (&str, &str) = (
    "rapidgator.limit_reached",
    "Rapidgator traffic or storage limit was reached",
);

/// `user/login` reported "Please wait" (JD: frequent-login flood protection).
pub(crate) const LOGIN_THROTTLED: (&str, &str) = (
    "rapidgator.login_throttled",
    "Rapidgator login was throttled; too many recent login attempts",
);

/// The API reported "Denied by IP" — JD treats this as a temporary account block, not a
/// permanently invalid credential (`AccountUnavailableException`, not `AccountInvalidException`).
pub(crate) const IP_DENIED: (&str, &str) = (
    "rapidgator.ip_denied",
    "Rapidgator denied the request from this IP address",
);

/// The API reported "User is not PREMIUM" / "This file can be downloaded by premium only" /
/// "You can download files up to ..." — the account is not premium.
pub(crate) const PREMIUM_REQUIRED: (&str, &str) = (
    "rapidgator.premium_required",
    "Rapidgator premium account is required to use the download API",
);

/// The API reported "Error: You requested login to your account from unusual Ip address" — JD
/// throws `AccountUnavailableException(msg, 60_000)`, the same temporary-block exception class
/// used for [`IP_DENIED`] and [`LIMIT_REACHED`] above, so this is mapped `RateLimited` too rather
/// than `AccountInvalid`: a scheduler retry after the account owner confirms the e-mail resolves
/// it, and nothing here can distinguish "confirmation pending" from "confirmed" ahead of time.
pub(crate) const IP_CONFIRMATION_REQUIRED: (&str, &str) = (
    "rapidgator.ip_confirmation_required",
    "Rapidgator requires confirming this login via the e-mail sent to the account",
);

/// The API reported a session-related error this plugin should never see (it logs in fresh on
/// every call and never reuses a cached `token`) — envelope `status` 401 not otherwise
/// classified, or a message mentioning "Session not exist"/"Session doesn't exist"/"This
/// download session is not for you"/"Session not found".
pub(crate) const SESSION_INVALID: (&str, &str) = (
    "rapidgator.session_invalid",
    "Rapidgator reported the login session as invalid",
);

/// HTTP 5xx, envelope `status` 500/503, or a network-level failure.
pub(crate) const SERVER_ERROR: (&str, &str) =
    ("rapidgator.server_error", "Rapidgator server error");

/// Bare HTTP 429 the JSON envelope didn't otherwise explain.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "rapidgator.rate_limited",
    "Rapidgator API rate limit was triggered",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("rapidgator.invalid_response", "Invalid Rapidgator response");

/// `user/login` reported no error but omitted the `response.token` field.
pub(crate) const NO_TOKEN: (&str, &str) = (
    "rapidgator.no_token",
    "Rapidgator did not return a login token",
);

/// `file/download` reported no error but omitted the `response.download_url` field.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "rapidgator.no_download_url",
    "Rapidgator did not return a download URL",
);

// --- account-less (free) website flow ---------------------------------------------------------

/// The file page carried none of the three markers the free flow needs (`var startTimerUrl`,
/// `var fid`, `var secs`); carries a `diagnosis`. Mirrors JD's
/// `if (startTimerUrl == null || fid == null || waitSecondsStr == null)` branch
/// (`RapidGatorNet.java:525`), which checks for a reason and then reports a plugin defect.
pub(crate) const NO_FREE_MARKERS: &str = "rapidgator.no_free_markers";

/// The free flow reached its last step but no page carried a download link; carries a
/// `diagnosis`.
pub(crate) const NO_FREE_LINK: &str = "rapidgator.no_free_link";

/// The page carried a download link on a host that does not belong to this hoster — refused
/// rather than followed. Carries the rejected `host`.
pub(crate) const FREE_LINK_HOST_MISMATCH: &str = "rapidgator.free_link_host_mismatch";

/// This IP may not start another free download yet; carries `wait_seconds` when the page stated
/// one. Mirrors the `ERROR_IP_BLOCKED` branches of JD's `handleErrorsWebsite`.
pub(crate) const FREE_LIMIT_REACHED: &str = "rapidgator.free_limit_reached";

/// The server-side countdown did not start (`{"state":"..."}` was not `"started"`); carries the
/// reported `state`. JD: "Error in pre download step #1" (`RapidGatorNet.java:545`), a temporary
/// failure it describes as "a very very rare case".
pub(crate) const TIMER_NOT_STARTED: &str = "rapidgator.timer_not_started";

/// `AjaxGetDownloadLink` did not report `"done"`; carries the reported `state`. JD: "Error in pre
/// download step #2" (`RapidGatorNet.java:585`).
pub(crate) const DOWNLOAD_LINK_NOT_READY: &str = "rapidgator.download_link_not_ready";

/// The hoster rejected the captcha answer ("Please fix the following input errors" / "The
/// verification code is incorrect").
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "rapidgator.captcha_rejected",
    "Rapidgator rejected the captcha answer",
);

pub(crate) fn no_free_markers(diagnosis: &str) -> String {
    format!("Rapidgator free download page was not recognized: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("Rapidgator free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_link_host_mismatch(host: &str) -> String {
    format!("Rapidgator returned a download link on an unexpected host: {host}")
}

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!(
            "Rapidgator free download limit reached; another download is possible in {seconds}s"
        ),
        None => "Rapidgator free download limit reached for this IP address".to_owned(),
    }
}

pub(crate) fn timer_not_started(state: &str) -> String {
    format!("Rapidgator did not start the pre-download countdown (state: {state})")
}

pub(crate) fn download_link_not_ready(state: &str) -> String {
    format!("Rapidgator did not finish the pre-download countdown (state: {state})")
}

/// The API envelope reported an error not covered by a specific code above; carries the
/// provider `api_status`/`message`. JD's own fallback for an unrecognized error treats it as
/// transient (retry after 60 seconds), not permanent.
pub(crate) const API_ERROR: &str = "rapidgator.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "rapidgator.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "rapidgator.invalid_url";

pub(crate) fn api_error(status: i64, message: &str) -> String {
    format!("Rapidgator API ({status}): {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("Rapidgator HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

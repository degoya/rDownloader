//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "keep2share.account_missing",
    "Keep2Share account is missing",
);

/// The account has no Keep2Share password configured.
pub(crate) const PASSWORD_MISSING: (&str, &str) = (
    "keep2share.password_missing",
    "Keep2Share password is missing",
);

/// The URL is not a Keep2Share file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) = (
    "keep2share.unsupported_link",
    "Not a supported Keep2Share link",
);

/// `/login` reported errorcode 41/70/72 ("Invalid username/password", the legacy alias, or
/// "Account banned" — JD groups all three into one `AccountInvalidException` case, see
/// `api::classify_error`'s doc comment), 74 ("Unknown login error", also `AccountInvalidException`
/// when an account is present) or 76 ("Account stolen"), or a bare HTTP 401/403.
pub(crate) const BAD_CREDENTIALS: (&str, &str) = (
    "keep2share.bad_credentials",
    "Keep2Share login or password is wrong",
);

/// `/login` reported errorcode 30 (image captcha) or 33 (reCaptcha) — JD detects this directly
/// on the `/login` response (before its generic error switch even runs) and drives an interactive
/// captcha-solve loop; this plugin has no captcha-solving capability, so it reports this
/// unconditionally rather than attempting one. Also covers errorcode 31 (captcha answer
/// rejected) and a captcha demand from any other endpoint (JD's own switch sends those to a
/// generic "plugin defect" outside of `/login`, since JD never expects a captcha there in premium
/// mode either) — folded into the same bucket since this plugin can act on it no differently
/// either way. Not retryable today; that is expected (see the task brief).
pub(crate) const LOGIN_CAPTCHA: (&str, &str) = (
    "keep2share.login_captcha",
    "Keep2Share requires solving a captcha",
);

/// `/geturl`/`/accountinfo`/`/getfilesinfo` reported errorcode 20 ("File not found") or 23
/// ("file_id is folder"), a `message` of "File not available" (checked before the errorcode
/// switch, so it wins regardless of the accompanying code), or a `/getfilesinfo` batch entry
/// whose id is absent from the response / carries `is_available: false` or `isDeleted: true`.
pub(crate) const FILE_OFFLINE: (&str, &str) = (
    "keep2share.file_offline",
    "Keep2Share file is not available",
);

/// Errorcode 3 ("Free user can't download large files"), 7 ("This download available only for
/// premium users"), 9 ("only for store subscribers") or 11/42's default (no sub-error) case
/// ("auth_token has expired" / generic download-not-available) — the account cannot download this
/// file without (further) premium access.
pub(crate) const PREMIUM_REQUIRED: (&str, &str) = (
    "keep2share.premium_required",
    "Keep2Share premium account is required to download this file",
);

/// Errorcode 2: "Traffic limit exceed" — `ipBlockedOrAccountLimit` waits
/// `FREE_RECONNECTWAIT_MILLIS` (1 hour) before a retry.
pub(crate) const TRAFFIC_EXHAUSTED: (&str, &str) = (
    "keep2share.traffic_exhausted",
    "Keep2Share traffic limit was reached",
);

/// Errorcode 1: "You've downloaded the maximum amount of files!" — same 1-hour wait as
/// [`TRAFFIC_EXHAUSTED`] (`ipBlockedOrAccountLimit(..., FREE_RECONNECTWAIT_MILLIS)`), but a
/// distinct server message/meaning (a download-count cap, not a byte-traffic cap).
pub(crate) const DOWNLOAD_LIMIT_REACHED: (&str, &str) = (
    "keep2share.download_limit_reached",
    "Keep2Share download limit was reached",
);

/// Errorcode 5: "Please wait to download this file" — JD reads the wait duration from the
/// sub-error's `timeRemaining` field (seconds, possibly fractional; truncated), defaulting to 15
/// minutes when absent/unparseable.
pub(crate) const DOWNLOAD_WAIT: (&str, &str) = (
    "keep2share.download_wait",
    "Keep2Share requires waiting before this file can be downloaded",
);

/// Errorcode 6: "Free account does not allow to download more than one file at the same time" —
/// a fixed 15-minute wait.
pub(crate) const TOO_MANY_PARALLEL: (&str, &str) = (
    "keep2share.too_many_parallel",
    "Keep2Share does not allow more parallel downloads on this account",
);

/// Errorcode 4: "You no can access to this file" — JD: `PluginException(ERROR_FATAL, ...)`, no
/// retry.
pub(crate) const NO_ACCESS: (&str, &str) = (
    "keep2share.no_access",
    "Keep2Share denied access to this file",
);

/// Errorcode 8: "This is private file" ('PRIVATE_ONLY') — the file can only be downloaded by its
/// owner; JD: `privateDownloadRestriction` → `PluginException(ERROR_FATAL, ...)`, no retry.
pub(crate) const PRIVATE_FILE: (&str, &str) = (
    "keep2share.private_file",
    "Keep2Share file can only be downloaded by its owner",
);

/// Errorcode 10 ("You are not authorized for this action" — bad/expired `auth_token`) or 75
/// ("This token not allow access from this IP address"). JD dumps the cached token and retries
/// after 1 minute rather than invalidating the account outright, since the login credentials
/// themselves can still be valid — this plugin never caches a token either way (fresh login every
/// call), so a fresh `/login` on the next attempt is the natural retry.
pub(crate) const SESSION_INVALID: (&str, &str) = (
    "keep2share.session_invalid",
    "Keep2Share reported the auth token as invalid",
);

/// Errorcode 21/22 with no sub-error entry to unwrap (or a sub-error whose own code isn't one of
/// the other known cases) — JD: `PluginException(ERROR_TEMPORARILY_UNAVAILABLE, ...)`, no
/// explicit wait (server default retry).
pub(crate) const TEMPORARILY_UNAVAILABLE: (&str, &str) = (
    "keep2share.temporarily_unavailable",
    "Keep2Share reported the download as temporarily unavailable",
);

/// Errorcode 73: "You can not access k2s.cc from your current network connection!" — JD waits 6
/// hours before retrying an authenticated account.
pub(crate) const NETWORK_RESTRICTED: (&str, &str) = (
    "keep2share.network_restricted",
    "Keep2Share denied access from this network connection",
);

/// The `errorCode` string enum `"captcha_need_wait"`/`"captcha_need_wait_daily"` (an IP
/// temporarily/permanently banned for too many captcha errors), errorcode 71 ("Login attempt was
/// exceed"), or a bare HTTP 429. Distinct retry delays per trigger — see
/// `api::classify_error`/`api::ensure_http_status`.
pub(crate) const FLOOD: (&str, &str) = (
    "keep2share.flood",
    "Keep2Share is throttling requests from this account or IP",
);

/// Bare HTTP 400/5xx, or the JSON body wasn't the expected shape at all.
pub(crate) const SERVER_ERROR: (&str, &str) =
    ("keep2share.server_error", "Keep2Share server error");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("keep2share.invalid_response", "Invalid Keep2Share response");

/// `/login` reported no error but omitted the `auth_token` field.
pub(crate) const NO_TOKEN: (&str, &str) = (
    "keep2share.no_token",
    "Keep2Share did not return a login token",
);

/// `/geturl` reported no error but omitted the `url` field.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "keep2share.no_download_url",
    "Keep2Share did not return a download URL",
);

/// Errorcode 31 ("ERROR_CAPTCHA_INVALID"): the hoster rejected the captcha answer the
/// account-less free flow submitted. IMPL-VERIFY (`K2SApi.java` rev 53214, lines 1540-1542):
/// JD throws `PluginException(LinkStatus.ERROR_CAPTCHA)` for this code, which is a *rejected*
/// answer rather than a captcha *demand* — hence a code of its own instead of
/// [`LOGIN_CAPTCHA`]'s (30/33, "a captcha is required"). Unreachable on the premium path,
/// which never submits a captcha answer.
pub(crate) const CAPTCHA_REJECTED: (&str, &str) = (
    "keep2share.captcha_rejected",
    "Keep2Share rejected the captcha answer",
);

/// `POST /requestcaptcha` answered without a usable `challenge`/`captcha_url` pair, or the
/// image URL did not serve an image at all. IMPL-VERIFY (`K2SApi.java:858-866`): JD logs the
/// two values and treats the same case as a plugin defect.
pub(crate) const CAPTCHA_UNAVAILABLE: (&str, &str) = (
    "keep2share.captcha_unavailable",
    "Keep2Share did not return a solvable captcha challenge",
);

/// The account-less free flow hit a download-count/traffic/wait limit that no captcha and no
/// countdown can work around. IMPL-VERIFY (`K2SApi.java:1013-1019` `ipBlockedOrAccountLimit`,
/// reached from errorcodes 1/2/5/6 and from a `time_wait` above 180 seconds): with
/// `account == null` JD raises `LinkStatus.ERROR_IP_BLOCKED`, so this is reported as
/// `IpBlocked` and the scheduler holds back every other free Keep2Share link instead of
/// burning a wait and a paid captcha on each of them. Carries `wait_seconds` when the API
/// stated one, plus the provider `message`.
pub(crate) const FREE_LIMIT_REACHED: &str = "keep2share.free_limit_reached";

/// The free flow ran to its end but `/geturl` never returned a `url`; carries a `diagnosis`
/// (the API's own `message`, when it sent one).
pub(crate) const NO_FREE_LINK: &str = "keep2share.no_free_link";

/// The API envelope reported an error not covered by a specific code above (JD's own default
/// `switch` arm — an unrecognized errorcode); carries the provider `api_status`/`message`.
pub(crate) const API_ERROR: &str = "keep2share.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "keep2share.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "keep2share.invalid_url";

pub(crate) fn free_limit_reached(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!(
            "Keep2Share free download limit reached; another download is possible in {seconds}s"
        ),
        None => "Keep2Share free download limit reached for this IP address".to_owned(),
    }
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("Keep2Share free download did not yield a file link: {diagnosis}")
}

pub(crate) fn api_error(status: i64, message: &str) -> String {
    format!("Keep2Share API ({status}): {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("Keep2Share HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

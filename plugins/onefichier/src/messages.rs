//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The account has no 1fichier API key configured.
pub(crate) const API_KEY_MISSING: (&str, &str) =
    ("1fichier.api_key_missing", "1fichier API key is missing");

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("1fichier.account_missing", "1fichier account is missing");

/// Account label part for `offer` `2`, the "Access" plan only 1fichier offers.
pub(crate) const PLAN_ACCESS: (&str, &str) = ("1fichier.account.access", "Access plan");

/// The URL is not a 1fichier file link.
pub(crate) const UNSUPPORTED_LINK: (&str, &str) =
    ("1fichier.unsupported_link", "Not a supported 1fichier link");

/// API responded `"Not authenticated"` / `"No such user"`, or HTTP 401/403.
pub(crate) const BAD_API_KEY: (&str, &str) = (
    "1fichier.bad_api_key",
    "1fichier API key is invalid or has expired",
);

/// API responded `"Resource not found"`, or HTTP 404.
pub(crate) const FILE_OFFLINE: (&str, &str) =
    ("1fichier.file_offline", "1fichier file is not available");

/// API flood protection was triggered; safe to retry after a cooldown.
pub(crate) const FLOOD: (&str, &str) = (
    "1fichier.flood",
    "1fichier API rate limit (flood protection) was triggered",
);

/// API responded `"Must be a customer"`: the API key belongs to a free account.
pub(crate) const PREMIUM_REQUIRED: (&str, &str) = (
    "1fichier.premium_required",
    "1fichier premium account is required to use the download API",
);

/// HTTP 5xx or a network-level failure.
pub(crate) const SERVER_ERROR: (&str, &str) = ("1fichier.server_error", "1fichier server error");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("1fichier.invalid_response", "Invalid 1fichier response");

/// `download/get_token.cgi` reported no error but omitted the `url` field.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "1fichier.no_download_url",
    "1fichier did not return a download URL",
);

/// The account-less website flow found no download form on the file page — the page is not a
/// download page at all. Carries a `diagnosis` (the page's own title/heading) so an unmapped
/// page still reaches the user in its own words. IMPL-VERIFY (`OneFichierCom.java` rev 53205,
/// lines 628-631): JD treats a missing `br.getForm(0)` as a plugin defect.
pub(crate) const NO_FREE_FORM: &str = "1fichier.no_free_form";

/// The form was posted but the answer carried no download link; carries a `diagnosis`.
/// IMPL-VERIFY (lines 696-703, 711-713): the same dead end JD reports.
pub(crate) const NO_FREE_LINK: &str = "1fichier.no_free_link";

/// This IP may not start another free download yet. IMPL-VERIFY (lines 820-866): with
/// `account == null` — the account-less flow's only case — every one of JD's wait/limit markers
/// ends in `LinkStatus.ERROR_IP_BLOCKED`, so this is reported as `IpBlocked` and the scheduler
/// holds back every other free 1fichier link instead of walking each one into the same wait.
/// Carries `wait_seconds`.
pub(crate) const FREE_LIMIT_REACHED: &str = "1fichier.free_limit_reached";

/// "Free download is temporarily limited due to high demand" — the hoster itself has no free
/// slot right now; nothing about this link or this IP is wrong. IMPL-VERIFY
/// (`isErrorNoFreeSlots`, lines 868-874, and `errorNoFreeSlots`, lines 876-897).
pub(crate) const NO_FREE_SLOTS: (&str, &str) = (
    "1fichier.no_free_slots",
    "1fichier has no free download slot available right now",
);

/// The file is password-protected. IMPL-VERIFY (`isPasswordProtectedFileWebsite`, lines
/// 542-550): JD prompts the user for the password and submits it as the form's `pass` field;
/// this plugin has no download password to send, so it reports the file instead.
pub(crate) const PASSWORD_REQUIRED: (&str, &str) = (
    "1fichier.password_required",
    "1fichier file is password-protected, which the account-less download does not support",
);

/// The file cannot be downloaded without an account at all. IMPL-VERIFY (lines 727-728 "not
/// possible to free unregistered users" -> `AccountRequiredException`, and lines 738-740, the
/// owner having reserved the file for subscribers).
pub(crate) const ACCOUNT_REQUIRED: (&str, &str) = (
    "1fichier.account_required",
    "1fichier requires an account to download this file",
);

/// The API envelope reported an error not covered by a specific code above; carries the
/// provider `message`.
pub(crate) const API_ERROR: &str = "1fichier.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "1fichier.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "1fichier.invalid_url";

pub(crate) fn no_free_form(diagnosis: &str) -> String {
    format!("1fichier free download form was not found: {diagnosis}")
}

pub(crate) fn no_free_link(diagnosis: &str) -> String {
    format!("1fichier free download did not yield a file link: {diagnosis}")
}

pub(crate) fn free_limit_reached(seconds: u64) -> String {
    format!("1fichier free download limit reached; another download is possible in {seconds}s")
}

pub(crate) fn api_error(message: &str) -> String {
    format!("1fichier API: {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("1fichier HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

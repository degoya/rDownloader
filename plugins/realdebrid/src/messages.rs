//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text, and
//! every code here has a line in `locales/{de,en,es,fr}.json`.
//!
//! Nothing a provider wrote appears in any of them. Real-Debrid answers a failure with
//! `{"error": "<sentence>", "error_code": <number>}`, and the sentence is the part that could
//! one day carry something it should not; the number is the part that is stable enough to
//! translate. So the number travels as the `api_code` parameter and the sentence is dropped —
//! the same rule `sanitize_error` applies in `realdebrid-auth`, arrived at from the other side.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The account holds no Real-Debrid access token: it has never been signed in, or the sign-in
/// was revoked and the renewal sweep could not replace it.
pub(crate) const TOKEN_MISSING: (&str, &str) = (
    "realdebrid.token_missing",
    "Real-Debrid account is not signed in",
);

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "realdebrid.account_missing",
    "Real-Debrid account is missing",
);

/// `error_code` 8/9/12/13/14/15, or HTTP 401/403 with nothing else to read.
pub(crate) const AUTH_INVALID: (&str, &str) = (
    "realdebrid.auth_invalid",
    "Real-Debrid sign-in is invalid or expired",
);

/// `error_code` 10/11: the account asks for a second factor this plugin cannot supply.
pub(crate) const TWO_FACTOR: (&str, &str) = (
    "realdebrid.two_factor",
    "Real-Debrid asked for two-factor authentication",
);

/// `error_code` 7/24/35, or HTTP 404/410/451.
pub(crate) const FILE_OFFLINE: (&str, &str) = (
    "realdebrid.file_offline",
    "Real-Debrid reports this file as unavailable",
);

/// `error_code` 16/20: this hoster is not covered, or not for this account's plan.
pub(crate) const HOST_UNSUPPORTED: (&str, &str) = (
    "realdebrid.host_unsupported",
    "Real-Debrid does not support this host for this account",
);

/// `error_code` 6/17/19/21/25: the provider or the hoster behind it is busy right now.
pub(crate) const SERVER_BUSY: (&str, &str) = (
    "realdebrid.server_busy",
    "Real-Debrid or the hoster behind it is temporarily unavailable",
);

/// `error_code` 18/23/36: a quota of the account or of the hoster was used up.
pub(crate) const LIMIT_REACHED: (&str, &str) = (
    "realdebrid.limit_reached",
    "Real-Debrid reports the traffic or hoster limit as reached",
);

/// `error_code` 22: this address may not use the account.
pub(crate) const IP_NOT_ALLOWED: (&str, &str) = (
    "realdebrid.ip_not_allowed",
    "Real-Debrid does not allow this account from this address",
);

/// `error_code` 5 or 34, or HTTP 429. The API is capped at 250 requests a minute and refused
/// requests count towards that cap, so waiting is the only correct answer.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "realdebrid.rate_limited",
    "Real-Debrid API rate limit was reached",
);

/// `unrestrict/link` reported no error but omitted `download`.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "realdebrid.no_download_url",
    "Real-Debrid did not return a download link",
);

/// HTTP 5xx with nothing else to read.
pub(crate) const SERVER_ERROR: (&str, &str) =
    ("realdebrid.server_error", "Real-Debrid server error");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "realdebrid.invalid_response",
    "Invalid Real-Debrid response",
);

/// An `error_code` not covered by a specific code above; carries the number as `api_code`.
pub(crate) const API_ERROR: &str = "realdebrid.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "realdebrid.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "realdebrid.invalid_url";

pub(crate) fn api_error(api_code: i64) -> String {
    format!("Real-Debrid API error {api_code}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("Real-Debrid HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

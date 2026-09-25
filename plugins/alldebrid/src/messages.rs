//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The account has no AllDebrid API key configured.
pub(crate) const API_KEY_MISSING: (&str, &str) =
    ("alldebrid.api_key_missing", "AllDebrid API key is missing");

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("alldebrid.account_missing", "AllDebrid account is missing");

/// AllDebrid has no `/link/infos` batch-check endpoint (see `api.rs`'s IMPL-VERIFY note), so
/// `check()` is not implemented. The native `Resolver` trait (`rd_plugin_api::Resolver::check`)
/// has a default that reports exactly this `(code, message)` pair; the WIT `Guest` trait has no
/// such default (every export must be implemented), so `guest.rs` reproduces it explicitly via
/// this constant instead of a hand-copied literal that could drift from the trait default.
/// `native/tests.rs::check_defaults_to_unsupported` asserts the native adapter's (trait-default)
/// output equals this constant, which keeps the two in sync.
pub(crate) const CHECK_UNSUPPORTED: (&str, &str) = (
    "link.check_unsupported",
    "Link check is not supported by this resolver",
);

/// API responded `AUTH_MISSING_APIKEY`/`AUTH_BAD_APIKEY`/`AUTH_USER_BANNED`/`ACCOUNT_INVALID`,
/// or HTTP 401/403. The JSON-error path attaches the provider's raw code as an `api_code`
/// param (see `api::coded_with_provider_code`); the bare HTTP-status path (401/403 with no JSON
/// error envelope to read a code from) does not.
pub(crate) const AUTH_INVALID: (&str, &str) = (
    "alldebrid.auth_invalid",
    "AllDebrid API key is invalid or the account is banned",
);

/// API responded `LINK_DOWN`/`LINK_NOT_FOUND`/`LINK_ERROR`, or HTTP 404/410/451.
pub(crate) const LINK_DOWN: (&str, &str) =
    ("alldebrid.link_down", "AllDebrid file is not available");

/// API responded `LINK_HOST_NOT_SUPPORTED`/`LINK_HOST_UNAVAILABLE`/`LINK_HOST_FULL`/
/// `LINK_HOST_LIMIT_REACHED`/`USER_LINK_INVALID`: the target host is not currently usable
/// through this account.
pub(crate) const HOST_UNSUPPORTED: (&str, &str) = (
    "alldebrid.host_unsupported",
    "AllDebrid does not currently support this host",
);

/// API responded `MUST_BE_PREMIUM`/`FREE_TRIAL_LIMIT_REACHED`: a premium AllDebrid account is
/// required.
pub(crate) const PREMIUM_REQUIRED: (&str, &str) = (
    "alldebrid.premium_required",
    "AllDebrid premium account is required",
);

/// API responded `LINK_TEMPORARY_UNAVAILABLE`/`MAINTENANCE`/`AUTH_BLOCKED`, all mapped to a
/// fixed 5-minute retry (mirrors JD's `AccountUnavailableException(msg, 5 * 60 * 1000)`).
pub(crate) const TEMPORARILY_UNAVAILABLE: (&str, &str) = (
    "alldebrid.temporarily_unavailable",
    "AllDebrid is temporarily unavailable",
);

/// API responded `LINK_PASS_PROTECTED`. Password-retry flows are out of scope for this plugin.
pub(crate) const PASSWORD_PROTECTED: (&str, &str) = (
    "alldebrid.password_protected",
    "AllDebrid link is password protected",
);

/// `link/unlock`'s `data.delayed` id is still processing (`link/delayed` status `1`); safe to
/// retry shortly, per AllDebrid's documented 5-second polling recommendation.
pub(crate) const LINK_DELAYED: (&str, &str) = (
    "alldebrid.link_delayed",
    "AllDebrid is preparing this file on their servers",
);

/// `link/unlock` reported no error but omitted `data.link`.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "alldebrid.no_download_url",
    "AllDebrid did not return a download URL",
);

/// HTTP 429, not covered by a JSON error envelope.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "alldebrid.rate_limited",
    "AllDebrid API rate limit was triggered",
);

/// HTTP 5xx or a network-level failure.
pub(crate) const SERVER_ERROR: (&str, &str) = ("alldebrid.server_error", "AllDebrid server error");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("alldebrid.invalid_response", "Invalid AllDebrid response");

/// The API envelope reported an error not covered by a specific code above; carries `api_code`
/// and `message` parameters.
pub(crate) const API_ERROR: &str = "alldebrid.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "alldebrid.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "alldebrid.invalid_url";

pub(crate) fn api_error(api_code: &str, message: &str) -> String {
    format!("AllDebrid API ({api_code}): {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("AllDebrid HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}

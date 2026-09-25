//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! Nothing Put.io wrote appears in any of them. The API answers a refusal with
//! `{"error_type": "<word>", "error_message": "<sentence>"}`; the word is stable and
//! documented and travels as the `reason` parameter, the sentence is not and is dropped.
//! `putio_common::reason` is where that rule is enforced.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a Put.io file address.
pub const NOT_A_PUTIO_LINK: (&str, &str) = (
    "putio.not_a_putio_link",
    "This is not a Put.io file address",
);

/// The call carried no account identity, so there is no credential it could run as.
pub const ACCOUNT_MISSING: (&str, &str) = (
    "putio.account_missing",
    "A Put.io account is needed for this address",
);

/// The account holds no Put.io access token.
pub const TOKEN_MISSING: (&str, &str) = ("putio.token_missing", "Put.io account is not signed in");

/// HTTP 401, or `error_type` `INVALID_TOKEN` / `INVALID_GRANT`.
pub const AUTH_INVALID: (&str, &str) =
    ("putio.auth_invalid", "Put.io sign-in is invalid or expired");

/// HTTP 403: the token is good and the account may not do this.
pub const NOT_PERMITTED: (&str, &str) = (
    "putio.not_permitted",
    "Put.io does not allow this account to do that",
);

/// HTTP 404/410: the file is not in the account any more, or never was.
pub const FILE_NOT_FOUND: (&str, &str) =
    ("putio.file_not_found", "Put.io no longer holds this file");

/// The address names a folder. A folder has no bytes; its files have.
pub const IS_A_FOLDER: (&str, &str) = (
    "putio.is_a_folder",
    "This Put.io address is a folder and not a file",
);

/// HTTP 429, or the documented rate-limit `error_type`. Put.io states the moment the window
/// reopens in `X-RateLimit-Reset`, which the plugin turns into a wait.
pub const RATE_LIMITED: (&str, &str) = ("putio.rate_limited", "Put.io API rate limit reached");

/// HTTP 5xx.
pub const SERVER_ERROR: (&str, &str) = ("putio.server_error", "Put.io is temporarily unavailable");

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = ("putio.invalid_response", "Invalid Put.io response");

/// A refusal this build has no bucket for. The stable word travels as `reason`; Put.io's
/// sentence does not.
pub const API_ERROR: (&str, &str) = ("putio.api_error", "Put.io API error");

/// An HTTP status no error document explains.
pub const HTTP_ERROR: (&str, &str) = ("putio.http_error", "Put.io HTTP status");

/// The remaining storage an account has, shown beside it.
pub const DISK_FREE: &str = "putio.disk_free";

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("Put.io HTTP status {status}")
}

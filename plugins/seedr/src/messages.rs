//! User-facing texts and stable failure codes.
//!
//! Every code here exists in all four shipped catalogues under `locales/`. The English text
//! beside it is the last rung, for a build whose catalogue does not know the code yet — it is
//! never a second channel, and nothing Seedr wrote is ever repeated in it.

/// The address is not a Seedr file address at all.
pub const NOT_A_SEEDR_LINK: (&str, &str) = (
    "seedr.not_a_seedr_link",
    "This address is not a Seedr file address",
);

/// No account was named for a call that needs one.
pub const ACCOUNT_MISSING: (&str, &str) = (
    "seedr.account_missing",
    "This download needs a Seedr account",
);

/// The account has no password stored, so the request would go out with half a credential.
pub const PASSWORD_MISSING: (&str, &str) = (
    "seedr.password_missing",
    "This Seedr account has no password stored",
);

/// HTTP 401 or 403: Seedr refused the e-mail address and password.
pub const AUTH_INVALID: (&str, &str) = (
    "seedr.auth_invalid",
    "Seedr rejected this account's e-mail address or password",
);

/// HTTP 402, or a refusal naming the plan. Seedr's own documentation says the REST API is
/// "accessible only to relevant premium account types", so this is a first-class answer rather
/// than a stray status.
pub const PLAN_REQUIRED: (&str, &str) = (
    "seedr.plan_required",
    "The Seedr REST API needs a premium plan on this account",
);

/// HTTP 404/410: the file is not in the account any more, or never was.
pub const FILE_NOT_FOUND: (&str, &str) =
    ("seedr.file_not_found", "Seedr no longer holds this file");

/// HTTP 429.
pub const RATE_LIMITED: (&str, &str) = ("seedr.rate_limited", "Seedr request limit reached");

/// HTTP 5xx.
pub const SERVER_ERROR: (&str, &str) = ("seedr.server_error", "Seedr is temporarily unavailable");

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = ("seedr.invalid_response", "Invalid Seedr response");

/// A refusal this build has no bucket for. The code-shaped word travels as `reason`; Seedr's
/// prose never does.
pub const API_ERROR: (&str, &str) = ("seedr.api_error", "Seedr API error");

/// An HTTP status nothing in the answer explains.
pub const HTTP_ERROR: (&str, &str) = ("seedr.http_error", "Seedr HTTP status");

/// The storage the account has left, shown beside it.
pub const SPACE_FREE: &str = "seedr.space_free";

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("Seedr HTTP status {status}")
}

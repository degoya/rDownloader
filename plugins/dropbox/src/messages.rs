//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Dropbox wrote appears in any of them: an API error document carries an
//! `error_summary` and a `user_message` written for people, and repeating either would put a
//! provider's prose — and whatever it happened to quote — into a log line and into the
//! interface.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The request carried no account identity, so there is nothing to sign the call with.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("dropbox.account_missing", "Dropbox account is missing");

/// No token is stored for this account, or Dropbox refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "dropbox.sign_in_required",
    "This Dropbox account has to be signed in again",
);

/// The address is not a Dropbox file address.
pub(crate) const NOT_A_DROPBOX_LINK: (&str, &str) = (
    "dropbox.not_a_dropbox_link",
    "This is not a Dropbox file address",
);

/// The file is gone, the link was revoked, or the account cannot see it.
pub(crate) const FILE_NOT_FOUND: (&str, &str) = (
    "dropbox.file_not_found",
    "This Dropbox file could not be found",
);

/// The address names a folder, which the folder crawler lists rather than the resolver.
pub(crate) const IS_A_FOLDER: (&str, &str) =
    ("dropbox.is_a_folder", "This Dropbox address is a folder");

/// Dropbox will not serve these bytes: a Paper document, a restricted file, or an account
/// without permission.
pub(crate) const DOWNLOAD_NOT_PERMITTED: (&str, &str) = (
    "dropbox.download_not_permitted",
    "Dropbox does not allow this file to be downloaded",
);

/// The shared link refused access: it is password-protected and the password is missing or
/// wrong, or it was shared with somebody else.
pub(crate) const LINK_ACCESS_DENIED: (&str, &str) = (
    "dropbox.link_access_denied",
    "Dropbox denied access to this shared link",
);

/// Dropbox is rate limiting this app or this account.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "dropbox.rate_limited",
    "Dropbox is rate limiting this account",
);

/// Dropbox answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("dropbox.invalid_response", "Invalid Dropbox response");

/// Dropbox is away or answered 5xx.
pub(crate) const UNAVAILABLE: (&str, &str) =
    ("dropbox.unavailable", "Dropbox is temporarily unavailable");

/// A refusal Dropbox named with a reason this plugin has no case for. The reason travels as
/// the `reason` parameter — sanitised down to the shape an API reason has, never as free text.
pub(crate) const API_REFUSED: (&str, &str) = (
    "dropbox.api_refused",
    "Dropbox refused this request: {reason}",
);

//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Dropbox wrote appears in any of them.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a Dropbox folder address.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "dropbox_crawler.not_a_folder",
    "This is not a Dropbox folder address",
);

/// No token is stored for the account, or Dropbox refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "dropbox_crawler.sign_in_required",
    "Dropbox did not accept this account for that folder",
);

/// The folder was read and holds nothing that can be downloaded.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "dropbox_crawler.folder_empty",
    "This Dropbox folder holds no files that can be downloaded",
);

/// The folder could not be read: it is gone, it is not shared, or Dropbox refused.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "dropbox_crawler.folder_unreachable",
    "This Dropbox folder could not be read",
);

/// The shared folder link refused access: password-protected and the password is missing or
/// wrong, or shared with somebody else.
pub(crate) const LINK_ACCESS_DENIED: (&str, &str) = (
    "dropbox_crawler.link_access_denied",
    "Dropbox denied access to this shared folder link",
);

/// Dropbox is rate limiting this app or this account.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "dropbox_crawler.rate_limited",
    "Dropbox is rate limiting this account",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "dropbox_crawler.invalid_response",
    "Invalid Dropbox response",
);

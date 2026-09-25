//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Google wrote appears in any of them.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a Google Drive folder address.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "google_drive_crawler.not_a_folder",
    "This is not a Google Drive folder address",
);

/// No token is stored for the account, or Google refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "google_drive_crawler.sign_in_required",
    "Google did not accept this account for that folder",
);

/// The folder was read and holds nothing that can be downloaded.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "google_drive_crawler.folder_empty",
    "This Google Drive folder holds no files that can be downloaded",
);

/// The folder could not be read: it is gone, it is not shared, or Drive refused.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "google_drive_crawler.folder_unreachable",
    "This Google Drive folder could not be read",
);

/// Google is rate limiting this account.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "google_drive_crawler.rate_limited",
    "Google Drive is rate limiting this account",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "google_drive_crawler.invalid_response",
    "Invalid Google Drive response",
);

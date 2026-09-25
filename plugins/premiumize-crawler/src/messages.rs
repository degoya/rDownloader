//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a Premiumize folder or item address.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "premiumize_crawler.not_a_folder",
    "This is not a Premiumize folder or item address",
);

/// The account has no API key stored, or Premiumize refused it.
pub(crate) const API_KEY_REQUIRED: (&str, &str) = (
    "premiumize_crawler.api_key_required",
    "Premiumize did not accept this account for that folder",
);

/// The folder was read and holds no files at all.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "premiumize_crawler.folder_empty",
    "This Premiumize folder holds no files",
);

/// The folder could not be read: it is gone, it is not shared, or the API refused.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "premiumize_crawler.folder_unreachable",
    "This Premiumize folder could not be read",
);

/// Premiumize is rate limiting this account.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "premiumize_crawler.rate_limited",
    "Premiumize is rate limiting this account",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "premiumize_crawler.invalid_response",
    "Invalid Premiumize response",
);

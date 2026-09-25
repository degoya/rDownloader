//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing pCloud wrote appears in any of them; what travels instead is `result`, pCloud's own
//! decimal refusal number.
#![allow(dead_code)] // The tests and the guest use different subsets.

/// The address is not a pCloud folder or public link address.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "pcloud_crawler.not_a_folder",
    "This is not a pCloud folder or public link address",
);

/// No token is stored for the account, or neither pCloud installation accepted the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "pcloud_crawler.sign_in_required",
    "pCloud did not accept this account for that folder",
);

/// The folder was read and holds nothing that can be downloaded.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "pcloud_crawler.folder_empty",
    "This pCloud folder holds no files that can be downloaded",
);

/// The folder could not be read: it is gone, or pCloud refused.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "pcloud_crawler.folder_unreachable",
    "This pCloud folder could not be read (result {result})",
);

/// A public link pCloud refused: gone, expired, out of traffic, or password-protected.
pub(crate) const LINK_UNAVAILABLE: (&str, &str) = (
    "pcloud_crawler.link_unavailable",
    "pCloud will not open this public link (result {result})",
);

/// pCloud is rate limiting this application or this account.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "pcloud_crawler.rate_limited",
    "pCloud is rate limiting this account",
);

/// pCloud answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("pcloud_crawler.invalid_response", "Invalid pCloud response");

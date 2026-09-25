//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Box wrote appears in any of them.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a Box folder address or shared link.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "box_crawler.not_a_folder",
    "This is not a Box folder address",
);

/// No token is stored for the account, or Box refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "box_crawler.sign_in_required",
    "Box did not accept this account for that folder",
);

/// The folder was read and holds nothing that can be downloaded.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "box_crawler.folder_empty",
    "This Box folder holds no files that can be downloaded",
);

/// The folder could not be read: it is gone, the link was withdrawn, or Box refused.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "box_crawler.folder_unreachable",
    "This Box folder could not be read",
);

/// The account may not read this folder, or the shared link wants a password it was not given.
pub(crate) const ACCESS_DENIED: (&str, &str) = (
    "box_crawler.access_denied",
    "This account may not access this Box folder",
);

/// Box is rate limiting this account or this application.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "box_crawler.rate_limited",
    "Box is rate limiting this account",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("box_crawler.invalid_response", "Invalid Box response");

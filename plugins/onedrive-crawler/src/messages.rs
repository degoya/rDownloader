//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Microsoft wrote appears in any of them.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a OneDrive or SharePoint folder link.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "onedrive_crawler.not_a_folder",
    "This is not a OneDrive or SharePoint folder address",
);

/// No token is stored for the account, or Microsoft refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "onedrive_crawler.sign_in_required",
    "Microsoft did not accept this account for that folder",
);

/// The folder was read and holds nothing that can be downloaded.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "onedrive_crawler.folder_empty",
    "This OneDrive folder holds no files that can be downloaded",
);

/// The folder could not be read: it is gone, the link was withdrawn, or Graph refused.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "onedrive_crawler.folder_unreachable",
    "This OneDrive folder could not be read",
);

/// The account may not read this folder: the link needs a different account or tenant.
pub(crate) const ACCESS_DENIED: (&str, &str) = (
    "onedrive_crawler.access_denied",
    "This account may not access this OneDrive folder",
);

/// Microsoft is throttling this account or this tenant.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "onedrive_crawler.rate_limited",
    "OneDrive is rate limiting this account",
);

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "onedrive_crawler.invalid_response",
    "Invalid OneDrive response",
);

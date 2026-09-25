//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Microsoft wrote appears in any of them: a Graph error document carries a `message`
//! field written for a developer, and repeating it would put a provider's prose — and whatever
//! it happened to quote — into a log line and into the interface.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The request carried no account identity, so there is nothing to sign the call with.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("onedrive.account_missing", "OneDrive account is missing");

/// No token is stored for this account, or Microsoft refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "onedrive.sign_in_required",
    "This OneDrive account has to be signed in again",
);

/// The address is not a OneDrive or SharePoint file address.
pub(crate) const NOT_A_ONEDRIVE_LINK: (&str, &str) = (
    "onedrive.not_a_onedrive_link",
    "This is not a OneDrive or SharePoint file address",
);

/// The item is gone, the sharing link was withdrawn, or the account cannot see it.
pub(crate) const ITEM_NOT_FOUND: (&str, &str) = (
    "onedrive.item_not_found",
    "This OneDrive item could not be found",
);

/// The address names a folder, which the folder crawler lists rather than the resolver.
pub(crate) const IS_A_FOLDER: (&str, &str) =
    ("onedrive.is_a_folder", "This OneDrive address is a folder");

/// The item is neither a file nor a folder — a OneNote notebook, which is a package.
pub(crate) const NOT_A_FILE: (&str, &str) = (
    "onedrive.not_a_file",
    "This OneDrive item is not a file that can be downloaded",
);

/// The account may not read this item: the link needs a different account or tenant, or the
/// share was made for somebody else.
pub(crate) const ACCESS_DENIED: (&str, &str) = (
    "onedrive.access_denied",
    "This account may not access this OneDrive item",
);

/// The item may be viewed but not downloaded — a SharePoint policy blocks the download.
pub(crate) const DOWNLOAD_NOT_PERMITTED: (&str, &str) = (
    "onedrive.download_not_permitted",
    "OneDrive does not allow this item to be downloaded",
);

/// Microsoft's scan flagged the file and will not serve it.
pub(crate) const MALWARE_DETECTED: (&str, &str) = (
    "onedrive.malware_detected",
    "OneDrive flagged this file as malware and will not serve it",
);

/// Graph could not make sense of the address — a sharing link it cannot decode, or one that
/// belongs to a tenant this account cannot reach.
pub(crate) const INVALID_REQUEST: (&str, &str) = (
    "onedrive.invalid_request",
    "OneDrive could not resolve this sharing link",
);

/// Microsoft is throttling this account or this tenant.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "onedrive.rate_limited",
    "OneDrive is rate limiting this account",
);

/// Graph answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("onedrive.invalid_response", "Invalid OneDrive response");

/// Graph is away or answered 5xx.
pub(crate) const UNAVAILABLE: (&str, &str) = (
    "onedrive.unavailable",
    "OneDrive is temporarily unavailable",
);

/// A refusal Graph named with a code this plugin has no case for. The code travels as the
/// `reason` parameter — sanitised down to the shape an API code has, never as free text.
pub(crate) const API_REFUSED: (&str, &str) = (
    "onedrive.api_refused",
    "OneDrive refused this request: {reason}",
);

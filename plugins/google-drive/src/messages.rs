//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Google wrote appears in any of them: an API error document carries a `message`
//! field written for a developer, and repeating it would put a provider's prose — and whatever
//! it happened to quote — into a log line and into the interface.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The request carried no account identity, so there is nothing to sign the call with.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "google_drive.account_missing",
    "Google Drive account is missing",
);

/// No token is stored for this account, or Google refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "google_drive.sign_in_required",
    "This Google Drive account has to be signed in again",
);

/// The address is not a Google Drive file address.
pub(crate) const NOT_A_DRIVE_LINK: (&str, &str) = (
    "google_drive.not_a_drive_link",
    "This is not a Google Drive file address",
);

/// The file is gone, was never shared, or the account cannot see it.
pub(crate) const FILE_NOT_FOUND: (&str, &str) = (
    "google_drive.file_not_found",
    "This Google Drive file could not be found",
);

/// The address names a folder, which the folder crawler lists rather than the resolver.
pub(crate) const IS_A_FOLDER: (&str, &str) = (
    "google_drive.is_a_folder",
    "This Google Drive address is a folder",
);

/// The owner switched downloading off, or the account only has view rights.
pub(crate) const DOWNLOAD_NOT_PERMITTED: (&str, &str) = (
    "google_drive.download_not_permitted",
    "Google Drive does not allow this file to be downloaded",
);

/// Google could not scan the file for viruses and will only serve it once that is acknowledged.
pub(crate) const VIRUS_SCAN_WARNING: (&str, &str) = (
    "google_drive.virus_scan_warning",
    "Google Drive could not scan this file for viruses",
);

/// The file's own download quota is used up — the usual answer for a widely shared link.
pub(crate) const QUOTA_EXCEEDED: (&str, &str) = (
    "google_drive.quota_exceeded",
    "This Google Drive file has exceeded its download quota",
);

/// Google is rate limiting this account or this file.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "google_drive.rate_limited",
    "Google Drive is rate limiting this account",
);

/// A Workspace document too large for Drive to export.
pub(crate) const EXPORT_TOO_LARGE: (&str, &str) = (
    "google_drive.export_too_large",
    "This Google Workspace document is too large for Drive to export",
);

/// A Workspace document type Drive exports nothing for.
pub(crate) const EXPORT_UNSUPPORTED: (&str, &str) = (
    "google_drive.export_unsupported",
    "Google Drive cannot export this kind of document",
);

/// The address asked for a format this document type does not export to.
pub(crate) const EXPORT_FORMAT_UNSUPPORTED: (&str, &str) = (
    "google_drive.export_format_unsupported",
    "Google Drive cannot export this document in the requested format",
);

/// Google answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "google_drive.invalid_response",
    "Invalid Google Drive response",
);

/// Google is away or answered 5xx.
pub(crate) const UNAVAILABLE: (&str, &str) = (
    "google_drive.unavailable",
    "Google Drive is temporarily unavailable",
);

/// A refusal Google named with a reason this plugin has no case for. The reason travels as the
/// `reason` parameter — sanitised down to the shape an API reason has, never as free text.
pub(crate) const API_REFUSED: (&str, &str) = (
    "google_drive.api_refused",
    "Google Drive refused this request: {reason}",
);

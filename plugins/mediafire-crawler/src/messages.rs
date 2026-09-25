//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing MediaFire wrote appears verbatim in any of them.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not a MediaFire folder address — or, for a bare key, turned out to be a
/// file: reported as `unsupported`, which hands the address on to the resolver.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "mediafire_crawler.not_a_folder",
    "This is not a MediaFire folder address",
);

/// The folder could not be read: it is gone, or the key is not a folder key.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "mediafire_crawler.folder_unreachable",
    "This MediaFire folder could not be read",
);

/// The folder is private, and this crawler signs nobody in.
pub(crate) const FOLDER_PRIVATE: (&str, &str) = (
    "mediafire_crawler.folder_private",
    "This MediaFire folder is private and needs the owner's account",
);

/// The folder was read and holds nothing that can be downloaded.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "mediafire_crawler.folder_empty",
    "This MediaFire folder holds no files that can be downloaded",
);

/// API error 261.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "mediafire_crawler.rate_limited",
    "MediaFire is rate limiting API calls from this address",
);

/// The API answered with something that is not the expected document.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "mediafire_crawler.invalid_response",
    "Invalid MediaFire response",
);

/// An API error this crawler has no closer name for; carries the sanitised `message`.
pub(crate) const API_ERROR: &str = "mediafire_crawler.api_error";

/// A status that is not an answer; carries `status`.
pub(crate) const HTTP_ERROR: &str = "mediafire_crawler.http_error";

pub(crate) fn api_error(message: &str) -> String {
    format!("MediaFire API: {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("MediaFire HTTP status {status}")
}

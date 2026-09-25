//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing MEGA answered with appears verbatim in any of them.
#![allow(dead_code)] // The host-side tests and the guest use different subsets.

/// Not a MEGA folder address. Reported as `unsupported`, which hands the address on.
pub(crate) const NOT_A_FOLDER: (&str, &str) = (
    "mega_crawler.not_a_folder",
    "This is not a MEGA folder address",
);

/// The fragment carries no 16-byte share key, so the listing cannot be opened.
pub(crate) const KEY_INVALID: (&str, &str) = (
    "mega_crawler.key_invalid",
    "This MEGA folder link carries no usable key",
);

/// MEGA answered `-9`: gone, or never there. The two are the same answer.
pub(crate) const FOLDER_UNREACHABLE: (&str, &str) = (
    "mega_crawler.folder_unreachable",
    "This MEGA folder no longer exists or never did",
);

/// MEGA answered `-11` or `-15`: this folder needs an account.
pub(crate) const FOLDER_PRIVATE: (&str, &str) = (
    "mega_crawler.folder_private",
    "This MEGA folder needs the owner's account",
);

/// MEGA answered `-3` or `-4`.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "mega_crawler.rate_limited",
    "MEGA is rate limiting requests from this address",
);

/// MEGA answered `-17`, or the transport answered `509`: the transfer quota is used up.
pub(crate) const QUOTA_EXCEEDED: (&str, &str) = (
    "mega_crawler.quota_exceeded",
    "MEGA's transfer quota for this folder is used up",
);

/// MEGA answered `-16` or `-18`.
pub(crate) const UNAVAILABLE: (&str, &str) = (
    "mega_crawler.unavailable",
    "MEGA is temporarily not answering for this folder",
);

/// The body was not the document a parser may read values out of.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("mega_crawler.invalid_response", "Invalid MEGA response");

/// The listing was read and holds no file whose key this link opens.
pub(crate) const FOLDER_EMPTY: (&str, &str) = (
    "mega_crawler.folder_empty",
    "This MEGA folder holds no files that can be downloaded",
);

/// The signed-in account's listing does not hold the named node (RD-120-30).
pub(crate) const NOT_IN_ACCOUNT: (&str, &str) = (
    "mega_crawler.not_in_account",
    "This folder is no longer in the MEGA account",
);

/// MEGA answered `-15` to a call made with the account's session: it has run out, and the
/// account has to be signed in again (RD-120-30).
pub(crate) const SESSION_EXPIRED: (&str, &str) = (
    "mega_crawler.session_expired",
    "The MEGA account's session has run out; sign it in again",
);

/// The listing held more files than one crawl returns; carries `limit`.
pub(crate) const TOO_MANY_FILES: &str = "mega_crawler.too_many_files";

/// An API error this crawler has no closer name for; carries `status`.
pub(crate) const API_ERROR: &str = "mega_crawler.api_error";

/// A status that is not an answer; carries `status`.
pub(crate) const HTTP_ERROR: &str = "mega_crawler.http_error";

pub(crate) fn too_many_files(limit: usize) -> String {
    format!("This MEGA folder holds more than {limit} files")
}

pub(crate) fn api_error(status: i64) -> String {
    format!("MEGA API error {status}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("MEGA HTTP status {status}")
}

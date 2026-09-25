//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address is not one this plugin can even try: it names a file, not a directory.
pub(crate) const NOT_A_DIRECTORY: (&str, &str) = (
    "directory_index_crawler.not_a_directory",
    "This address does not point at a directory listing",
);

/// The page was fetched and is not a directory listing. The one refusal that hands the
/// address on: the selection keeps looking rather than ending the link here.
pub(crate) const NOT_A_LISTING: (&str, &str) = (
    "directory_index_crawler.not_a_listing",
    "This page is not an open directory listing",
);

/// The listing was read and holds no files at all.
pub(crate) const DIRECTORY_EMPTY: (&str, &str) = (
    "directory_index_crawler.directory_empty",
    "This directory listing holds no files",
);

/// The server refused the listing: it is gone, it is closed, or it wants a sign-in.
pub(crate) const DIRECTORY_UNREACHABLE: (&str, &str) = (
    "directory_index_crawler.directory_unreachable",
    "This directory listing could not be read",
);

/// The server asked for credentials this plugin has none of.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "directory_index_crawler.sign_in_required",
    "This directory is not open: the server asked for a sign-in",
);

/// The server is rate limiting or temporarily unavailable.
pub(crate) const SERVER_BUSY: (&str, &str) = (
    "directory_index_crawler.server_busy",
    "The server is not answering this listing right now",
);

//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The address does not have the shape of a share link at all.
pub(crate) const NOT_A_SHARE: (&str, &str) = (
    "nextcloud_crawler.not_a_share",
    "This is not a Nextcloud or ownCloud share address",
);

/// The address has the shape but there is no Nextcloud behind it. The one refusal that hands
/// the address on: the selection keeps looking rather than ending the link here.
pub(crate) const NOT_A_NEXTCLOUD: (&str, &str) = (
    "nextcloud_crawler.not_a_nextcloud",
    "This address has the shape of a share, but no Nextcloud answered it",
);

/// The share is protected and no password was given with the address.
pub(crate) const PASSWORD_REQUIRED: (&str, &str) = (
    "nextcloud_crawler.password_required",
    "This share is password protected: add the password to the address after a # to open it",
);

/// A password was given and the server rejected it.
pub(crate) const PASSWORD_WRONG: (&str, &str) = (
    "nextcloud_crawler.password_wrong",
    "The server did not accept the password given with this share address",
);

/// The share exists and holds no files at all.
pub(crate) const SHARE_EMPTY: (&str, &str) =
    ("nextcloud_crawler.share_empty", "This share holds no files");

/// The share could not be read: it is gone, it expired, or the server refused.
pub(crate) const SHARE_UNREACHABLE: (&str, &str) = (
    "nextcloud_crawler.share_unreachable",
    "This share could not be read: it may have expired or been withdrawn",
);

/// The server is rate limiting or temporarily unavailable.
pub(crate) const SERVER_BUSY: (&str, &str) = (
    "nextcloud_crawler.server_busy",
    "The server is not answering this share right now",
);

/// The endpoint answered with something that is not a WebDAV listing.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "nextcloud_crawler.invalid_response",
    "Invalid WebDAV response from this server",
);

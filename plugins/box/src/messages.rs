//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing Box wrote appears in any of them: an API error document carries a `message` and a
//! `context_info` written for a developer, and repeating either would put a provider's prose —
//! and whatever it happened to quote — into a log line and into the interface.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The request carried no account identity, so there is nothing to sign the call with.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = ("box.account_missing", "Box account is missing");

/// No token is stored for this account, or Box refused the one that is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "box.sign_in_required",
    "This Box account has to be signed in again",
);

/// The address is not a Box file address.
pub(crate) const NOT_A_BOX_LINK: (&str, &str) =
    ("box.not_a_box_link", "This is not a Box file address");

/// The file is gone, the link was revoked, or the account cannot see it.
pub(crate) const FILE_NOT_FOUND: (&str, &str) =
    ("box.file_not_found", "This Box file could not be found");

/// The address names a folder, which the folder crawler lists rather than the resolver.
pub(crate) const IS_A_FOLDER: (&str, &str) = ("box.is_a_folder", "This Box address is a folder");

/// The item exists but is not a file: a bookmark, or something Box has no bytes for.
pub(crate) const NOT_A_FILE: (&str, &str) = ("box.not_a_file", "This Box item is not a file");

/// Box will not serve these bytes to this account.
pub(crate) const DOWNLOAD_NOT_PERMITTED: (&str, &str) = (
    "box.download_not_permitted",
    "Box does not allow this file to be downloaded",
);

/// The shared link refused access: it is password-protected and the password is missing or
/// wrong, or it was shared with somebody else.
pub(crate) const LINK_ACCESS_DENIED: (&str, &str) = (
    "box.link_access_denied",
    "Box denied access to this shared link",
);

/// The account is over its storage or download allowance.
pub(crate) const QUOTA_EXCEEDED: (&str, &str) = (
    "box.quota_exceeded",
    "This Box account has used up its allowance",
);

/// Box is rate limiting this application or this account.
pub(crate) const RATE_LIMITED: (&str, &str) =
    ("box.rate_limited", "Box is rate limiting this account");

/// Box answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = ("box.invalid_response", "Invalid Box response");

/// Box is away or answered 5xx.
pub(crate) const UNAVAILABLE: (&str, &str) = ("box.unavailable", "Box is temporarily unavailable");

/// A refusal Box named with a code this plugin has no case for. The code travels as the
/// `reason` parameter — sanitised down to the shape an API code has, never as free text.
pub(crate) const API_REFUSED: (&str, &str) =
    ("box.api_refused", "Box refused this request: {reason}");

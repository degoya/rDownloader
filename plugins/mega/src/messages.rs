//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! No value MEGA answered with is ever put in one of them: an attribute block, a storage
//! address and above all a key are none of a message's business.
#![allow(dead_code)] // The host-side tests and the guest use different subsets.

/// Not a MEGA file address. Reported as `unsupported`, which hands the link on.
pub const NOT_MINE: (&str, &str) = ("mega.not_mine", "This is not a MEGA file address");

/// The address carries no usable key, so nothing behind it can be decrypted.
pub const KEY_INVALID: (&str, &str) = (
    "mega.key_invalid",
    "This MEGA link carries no usable decryption key",
);

/// MEGA answered `-9`. Deleted and never existed are the same answer -- measured, not assumed.
pub const NOT_FOUND: (&str, &str) = (
    "mega.not_found",
    "This MEGA file no longer exists or never did",
);

/// MEGA answered `-11`: the caller may not see this node.
pub const ACCESS_DENIED: (&str, &str) = (
    "mega.access_denied",
    "This MEGA file needs an account that may see it",
);

/// MEGA answered `-15`: a session is required. A public link should never produce this.
pub const SESSION_REQUIRED: (&str, &str) = (
    "mega.session_required",
    "This MEGA file is only reachable while signed in",
);

/// MEGA answered `-3` or `-4`.
pub const RATE_LIMITED: (&str, &str) = (
    "mega.rate_limited",
    "MEGA is rate limiting requests from this address",
);

/// MEGA answered `-17`: the transfer quota is used up.
pub const QUOTA_EXCEEDED: (&str, &str) = (
    "mega.quota_exceeded",
    "MEGA's transfer quota for this address is used up",
);

/// MEGA answered `-16` or `-18`.
pub const UNAVAILABLE: (&str, &str) = (
    "mega.unavailable",
    "MEGA is temporarily not answering for this file",
);

/// The body was not the document a parser may read values out of.
pub const INVALID_RESPONSE: (&str, &str) = ("mega.invalid_response", "Invalid MEGA response");

/// The attribute block did not decrypt, which means the key does not belong to this file.
pub const ATTRIBUTES_UNREADABLE: (&str, &str) = (
    "mega.attributes_unreadable",
    "This MEGA link's key does not open this file",
);

/// The folder was read but does not hold the named file any more.
pub const NODE_MISSING: (&str, &str) = (
    "mega.node_missing",
    "This file is no longer in the MEGA folder it was found in",
);

/// The signed-in account's node list does not hold the named file (RD-120-30).
pub const ACCOUNT_NODE_MISSING: (&str, &str) = (
    "mega.account_node_missing",
    "This file is no longer in the MEGA account",
);

/// The file is in the account's listing but its key is not the account's own -- it came in
/// through somebody else's share, and opening it takes that share's key (RD-120-30).
pub const ACCOUNT_KEY_FOREIGN: (&str, &str) = (
    "mega.account_key_foreign",
    "This file reached the MEGA account through a share, which this plugin cannot open yet",
);

/// An API error this plugin has no closer name for; carries `status`.
pub const API_ERROR: &str = "mega.api_error";

/// A status that is not an answer; carries `status`.
pub const HTTP_ERROR: &str = "mega.http_error";

pub fn api_error(status: i64) -> String {
    format!("MEGA API error {status}")
}

pub fn http_error(status: u16) -> String {
    format!("MEGA HTTP status {status}")
}

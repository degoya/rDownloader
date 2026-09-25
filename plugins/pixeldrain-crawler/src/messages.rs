//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Pixeldrain's own `message` field is prose and never travels; only its `value` token does,
//! and only as the `api_code` parameter.
#![allow(dead_code)] // The tests and the guest use different subsets.

/// `list_not_found`, a 404 or a 410: the list was deleted or never existed.
pub const LIST_NOT_FOUND: (&str, &str) = (
    "pixeldrain_crawler.list_not_found",
    "This Pixeldrain list does not exist any more",
);

/// The list was read and names no file.
pub const LIST_EMPTY: (&str, &str) = (
    "pixeldrain_crawler.list_empty",
    "This Pixeldrain list holds no files",
);

/// Pixeldrain serves this list only to a signed-in account.
pub const ACCOUNT_REQUIRED: (&str, &str) = (
    "pixeldrain_crawler.account_required",
    "Pixeldrain serves this list only to a signed-in account",
);

/// This connection has spent its share of Pixeldrain's free allowance.
pub const RATE_LIMITED: (&str, &str) = (
    "pixeldrain_crawler.rate_limited",
    "Pixeldrain's request limit for this connection is reached. Waiting.",
);

/// A 5xx, or the provider's own `internal`.
pub const SERVER_ERROR: (&str, &str) = (
    "pixeldrain_crawler.server_error",
    "Pixeldrain did not answer: the list could not be read",
);

/// The answer is not the JSON this endpoint is documented to produce.
pub const INVALID_RESPONSE: (&str, &str) = (
    "pixeldrain_crawler.invalid_response",
    "Invalid Pixeldrain response",
);

/// A refusal this build has no bucket for. The provider's `value` travels as `api_code`.
pub const API_ERROR: (&str, &str) = ("pixeldrain_crawler.api_error", "Pixeldrain API error");

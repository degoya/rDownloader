//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! The English text here is a redaction-safe fallback for a backend that has no catalogue; what
//! a person reads is the translation. Pixeldrain's own `message` field is prose and never
//! travels -- only its `value` token does, and only as the `api_code` parameter.
#![allow(dead_code)] // The guest and the native tests use different subsets.

/// The address is on Pixeldrain and in no shape this plugin serves.
pub const UNSUPPORTED_LINK: (&str, &str) = (
    "pixeldrain.unsupported_link",
    "This is not a Pixeldrain file address",
);

/// The address did not parse at all.
pub const INVALID_LINK: (&str, &str) = ("pixeldrain.invalid_link", "Invalid Pixeldrain address");

/// `not_found`, a 404, or a 410. The file was deleted or never existed.
pub const FILE_NOT_FOUND: (&str, &str) = (
    "pixeldrain.file_not_found",
    "This file does not exist on Pixeldrain any more",
);

/// The file is there and Pixeldrain refuses to serve it: a moderation verdict.
pub const FILE_BLOCKED: (&str, &str) = (
    "pixeldrain.file_blocked",
    "Pixeldrain has blocked this file and does not serve it",
);

/// This IP has spent its share of Pixeldrain's free allowance.
pub const IP_RATE_LIMITED: (&str, &str) = (
    "pixeldrain.ip_rate_limited",
    "Pixeldrain's download limit for this connection is reached. Waiting.",
);

/// The transfer volume of the file or its uploader is exhausted for the period. A premium
/// subscription on either end is what lifts it.
pub const TRANSFER_LIMIT: (&str, &str) = (
    "pixeldrain.transfer_limit",
    "Pixeldrain's transfer limit for this file is reached. A Pixeldrain premium subscription on \
     either side lifts it.",
);

/// Too many downloads from this connection at once.
pub const TOO_MANY_DOWNLOADS: (&str, &str) = (
    "pixeldrain.too_many_downloads",
    "Pixeldrain allows no further download from this connection at the moment. Waiting.",
);

/// Pixeldrain puts a captcha in front of this file. This plugin declares no captcha capability.
pub const CAPTCHA_REQUIRED: (&str, &str) = (
    "pixeldrain.captcha_required",
    "Pixeldrain asks for a captcha for this file, which this plugin cannot answer",
);

/// The file is not public: Pixeldrain wants a signed-in account for it.
pub const ACCOUNT_REQUIRED: (&str, &str) = (
    "pixeldrain.account_required",
    "Pixeldrain serves this file only to a signed-in account",
);

/// A 5xx, or the provider's own `internal`.
pub const SERVER_ERROR: (&str, &str) = ("pixeldrain.server_error", "Pixeldrain server error");

/// `server_overload` in the quota answer: the service says it is busy before anything is asked
/// of it.
pub const SERVER_OVERLOADED: (&str, &str) = (
    "pixeldrain.server_overloaded",
    "Pixeldrain reports itself overloaded. Waiting.",
);

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) =
    ("pixeldrain.invalid_response", "Invalid Pixeldrain response");

/// A refusal this build has no bucket for. The provider's `value` travels as `api_code`; its
/// prose never does.
pub const API_ERROR: (&str, &str) = ("pixeldrain.api_error", "Pixeldrain API error");

/// An HTTP status nothing in the answer explains.
pub const HTTP_ERROR: (&str, &str) = ("pixeldrain.http_error", "Pixeldrain HTTP status");

/// The address this plugin built is not on a host the queue may fetch from.
pub const DIRECT_LINK_FOREIGN: (&str, &str) = (
    "pixeldrain.direct_link_foreign",
    "Pixeldrain named a download address outside its own domains",
);

/// An account exists but holds no API key, so there is nothing to check or send.
pub const API_KEY_MISSING: (&str, &str) = (
    "pixeldrain.api_key_missing",
    "This Pixeldrain account has no API key stored",
);

/// `authentication_failed`: Pixeldrain does not accept the stored API key.
pub const API_KEY_INVALID: (&str, &str) = (
    "pixeldrain.api_key_invalid",
    "Pixeldrain rejected this account's API key. It may have been revoked or expired.",
);

//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! Nothing Offcloud wrote appears in any of them. The API answers a refusal with
//! `{"error": "<sentence>"}`, and the sentence is not stable — it is prose, and prose that may
//! quote the address it was asked about. What travels is the one stable word the provider's
//! own clients branch on (`NOAUTH`), the closed set of `not_available` reasons, and the HTTP
//! status; everything else is dropped. The same rule the other multihoster plugins arrived at.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The account holds no Offcloud API key.
pub const API_KEY_MISSING: (&str, &str) = (
    "offcloud.api_key_missing",
    "This Offcloud account has no API key yet",
);

/// The call carried no account identity, so there is no credential it could run as.
pub const ACCOUNT_MISSING: (&str, &str) =
    ("offcloud.account_missing", "Offcloud account is missing");

/// `NOAUTH`, or HTTP 401/403 with nothing else to read.
pub const AUTH_INVALID: (&str, &str) = (
    "offcloud.auth_invalid",
    "The Offcloud API key is invalid or no longer valid",
);

/// Premium, and refused by Offcloud for a reason it does not name.
pub const DOWNLOAD_BLOCKED: (&str, &str) = (
    "offcloud.download_blocked",
    "Offcloud does not allow downloads on this account",
);

/// `not_available`: this link needs an add-on the account has not bought.
pub const ADDON_REQUIRED: (&str, &str) = (
    "offcloud.addon_required",
    "Offcloud needs an additional add-on for this link",
);

/// The link is not at the provider any more, or never was.
pub const LINK_GONE: (&str, &str) = ("offcloud.link_gone", "Offcloud no longer holds this link");

/// The call succeeded and Offcloud named no address to fetch.
pub const NO_DOWNLOAD_URL: (&str, &str) = (
    "offcloud.no_download_url",
    "Offcloud returned no download address",
);

/// The address Offcloud answered with is not one the queue may fetch.
pub const BAD_DOWNLOAD_URL: (&str, &str) = (
    "offcloud.bad_download_url",
    "Offcloud returned an unusable download address",
);

/// HTTP 429, or an exhausted allowance.
pub const RATE_LIMITED: (&str, &str) = (
    "offcloud.rate_limited",
    "The Offcloud request limit was reached",
);

/// A 5xx with nothing else to read.
pub const SERVER_ERROR: (&str, &str) = ("offcloud.server_error", "Offcloud server error");

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) =
    ("offcloud.invalid_response", "Invalid Offcloud response");

/// Offcloud has no batch link-status endpoint for hoster links.
pub const CHECK_UNSUPPORTED: (&str, &str) = (
    "offcloud.check_unsupported",
    "Offcloud cannot check links without starting them",
);

/// A refusal this build has no bucket for. The provider's word travels as `api_code` when it
/// is code-shaped; its prose never does.
pub const API_ERROR: (&str, &str) = ("offcloud.api_error", "Offcloud API error");

/// An HTTP status nothing in the answer explains.
pub const HTTP_ERROR: (&str, &str) = ("offcloud.http_error", "Offcloud HTTP status");

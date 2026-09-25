//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others. Nothing TorBox wrote appears in any of them: the `error`
//! word is stable and documented and travels as the `api_code` parameter, the `detail`
//! sentence is not and is dropped.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The call carried no account identity, so there is no credential it could run as.
pub const ACCOUNT_MISSING: (&str, &str) = ("torbox.account_missing", "TorBox account is missing");

/// The account holds no TorBox API key.
pub const KEY_MISSING: (&str, &str) = ("torbox.key_missing", "This TorBox account has no API key");

/// `BAD_TOKEN`, `AUTH_ERROR`, `NO_AUTH`, or HTTP 401/403 with nothing else to read.
pub const AUTH_INVALID: (&str, &str) = (
    "torbox.auth_invalid",
    "The TorBox API key is invalid or expired",
);

/// The address is not one this plugin can mint a download from.
pub const NOT_A_TICKET: (&str, &str) = (
    "torbox.not_a_ticket",
    "This is not a TorBox download address",
);

/// `ITEM_NOT_FOUND`, `LINK_OFFLINE`, HTTP 404: the job or the file is not there any more.
pub const FILE_GONE: (&str, &str) = ("torbox.file_gone", "TorBox no longer offers this file");

/// `PLAN_RESTRICTED_FEATURE`.
pub const NOT_PERMITTED: (&str, &str) = (
    "torbox.not_permitted",
    "The TorBox plan does not cover this download",
);

/// `MONTHLY_LIMIT` and its neighbours.
pub const LIMIT_REACHED: (&str, &str) = (
    "torbox.limit_reached",
    "TorBox reports the plan limit as reached",
);

/// HTTP 429. Refused requests count towards the very cap that refused them, so this is a wait
/// with a floor rather than an immediate retry.
pub const RATE_LIMITED: (&str, &str) = ("torbox.rate_limited", "TorBox API rate limit reached");

/// A 5xx, `DATABASE_ERROR`, `DOWNLOAD_SERVER_ERROR`, `NO_SERVERS_AVAILABLE_ERROR`.
pub const SERVER_BUSY: (&str, &str) = ("torbox.server_busy", "TorBox is temporarily unavailable");

/// HTTP 451, `INVALID_OPTION` and its neighbours.
pub const REQUEST_REFUSED: (&str, &str) = ("torbox.request_refused", "TorBox refused this request");

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = ("torbox.invalid_response", "Invalid TorBox response");

/// `requestdl` succeeded and named no address. There is nothing to fetch and nothing to say
/// about why, so it is reported rather than retried against an empty answer.
pub const NO_DOWNLOAD_URL: (&str, &str) = (
    "torbox.no_download_url",
    "TorBox did not return a download address",
);

/// A documented `error` word this build has no bucket for. The word travels; TorBox's sentence
/// does not.
pub const API_ERROR: (&str, &str) = ("torbox.api_error", "TorBox API error");

/// An HTTP status no `error` word explains.
pub const HTTP_ERROR: (&str, &str) = ("torbox.http_error", "TorBox HTTP status");

#[must_use]
pub fn api_error(api_code: &str) -> String {
    format!("TorBox API error {api_code}")
}

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("TorBox HTTP status {status}")
}

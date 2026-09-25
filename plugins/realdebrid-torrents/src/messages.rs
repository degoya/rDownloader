//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! Nothing Real-Debrid wrote appears in any of them. The API answers a refusal with
//! `{"error": "<sentence>", "error_code": <number>}`; the number is stable and documented and
//! travels as the `api_code` parameter, the sentence is not and is dropped. The same rule the
//! resolver sibling arrived at in `plugins/realdebrid/src/messages.rs`.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The source is neither a magnet naming a BitTorrent info hash nor a readable container.
pub const NOT_A_TORRENT: (&str, &str) = (
    "realdebrid_torrents.not_a_torrent",
    "This is not a magnet address or a torrent file",
);

/// `magnet_error`: Real-Debrid could not turn the magnet into a torrent at all.
pub const MAGNET_REJECTED: (&str, &str) = (
    "realdebrid_torrents.magnet_rejected",
    "Real-Debrid could not read this magnet address",
);

/// `dead`: no peer answered for long enough that Real-Debrid gave up.
pub const TORRENT_DEAD: (&str, &str) = (
    "realdebrid_torrents.torrent_dead",
    "Real-Debrid found no peers for this torrent",
);

/// `error`, and any other end Real-Debrid reports without a reason of its own.
pub const TORRENT_FAILED: (&str, &str) = (
    "realdebrid_torrents.torrent_failed",
    "Real-Debrid ended this torrent with an error",
);

/// `virus`, `error_code` 25/26, HTTP 451: Real-Debrid refuses to carry this content.
pub const CONTENT_REFUSED: (&str, &str) = (
    "realdebrid_torrents.content_refused",
    "Real-Debrid refuses to carry this content",
);

/// The torrent is not in the account any more, or never was.
pub const TORRENT_GONE: (&str, &str) = (
    "realdebrid_torrents.torrent_gone",
    "Real-Debrid no longer holds this torrent",
);

/// `error_code` 8-15, or HTTP 401/403 with nothing else to read.
pub const AUTH_INVALID: (&str, &str) = (
    "realdebrid_torrents.auth_invalid",
    "Real-Debrid sign-in is invalid or expired",
);

/// The account holds no Real-Debrid access token.
pub const TOKEN_MISSING: (&str, &str) = (
    "realdebrid_torrents.token_missing",
    "Real-Debrid account is not signed in",
);

/// The call carried no account identity, so there is no credential it could run as.
pub const ACCOUNT_MISSING: (&str, &str) = (
    "realdebrid_torrents.account_missing",
    "Real-Debrid account is missing",
);

/// `error_code` 16/20: this account's plan does not cover torrents.
pub const NOT_PERMITTED: (&str, &str) = (
    "realdebrid_torrents.not_permitted",
    "Real-Debrid does not allow torrents on this account",
);

/// `error_code` 6/17/19/21, or a 5xx: the provider is busy right now.
pub const SERVER_BUSY: (&str, &str) = (
    "realdebrid_torrents.server_busy",
    "Real-Debrid is temporarily unavailable",
);

/// A 5xx with nothing else to read.
pub const SERVER_ERROR: (&str, &str) = (
    "realdebrid_torrents.server_error",
    "Real-Debrid server error",
);

/// `error_code` 18/23/36: traffic or fair-use limit reached.
pub const LIMIT_REACHED: (&str, &str) = (
    "realdebrid_torrents.limit_reached",
    "Real-Debrid reports the traffic or torrent limit as reached",
);

/// `error_code` 22: the credential is good, the address it is used from is not.
pub const IP_NOT_ALLOWED: (&str, &str) = (
    "realdebrid_torrents.ip_not_allowed",
    "Real-Debrid does not allow this account from this address",
);

/// `error_code` 5/34, or HTTP 429. Refused requests count towards the very cap that refused
/// them, so this is a wait with a floor rather than an immediate retry.
pub const RATE_LIMITED: (&str, &str) = (
    "realdebrid_torrents.rate_limited",
    "Real-Debrid API rate limit reached",
);

/// A selection naming nothing. Never reached from the host, which refuses one before it gets
/// here; kept because at Real-Debrid an empty `files` value is an error and at other providers
/// it silently means "all of them", and neither is an answer anybody gave.
pub const EMPTY_CHOICE: (&str, &str) = (
    "realdebrid_torrents.empty_choice",
    "A file selection has to name at least one file",
);

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = (
    "realdebrid_torrents.invalid_response",
    "Invalid Real-Debrid response",
);

/// The submit succeeded and Real-Debrid named no torrent. Without an identifier there is
/// nothing to poll, nothing to choose against and nothing to delete.
pub const NO_TORRENT_ID: (&str, &str) = (
    "realdebrid_torrents.no_torrent_id",
    "Real-Debrid did not name the torrent it created",
);

/// The torrent finished and carries no link at all.
pub const NO_LINKS: (&str, &str) = (
    "realdebrid_torrents.no_links",
    "Real-Debrid reports this torrent as finished but returned no links",
);

/// A documented `error_code` this build has no bucket for. The number travels; the provider's
/// sentence does not.
pub const API_ERROR: (&str, &str) = ("realdebrid_torrents.api_error", "Real-Debrid API error");

/// An HTTP status no `error_code` explains.
pub const HTTP_ERROR: (&str, &str) = ("realdebrid_torrents.http_error", "Real-Debrid HTTP status");

#[must_use]
pub fn api_error(api_code: i64) -> String {
    format!("Real-Debrid API error {api_code}")
}

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("Real-Debrid HTTP status {status}")
}

//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! Nothing Put.io wrote appears in any of them. The API answers a refusal with
//! `{"error_type": "<WORD>", "error_message": "<sentence>"}`; the word is stable and
//! documented and travels as the `reason` parameter, the sentence is not and is dropped. The
//! same rule the resolver sibling follows, enforced in one place by `putio_common::reason`.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The source is neither a magnet naming a BitTorrent info hash nor a readable container.
pub const NOT_A_TORRENT: (&str, &str) = (
    "putio_transfers.not_a_torrent",
    "This is not a magnet address or a torrent file",
);

/// `ERROR`: Put.io ended the transfer itself.
pub const TRANSFER_FAILED: (&str, &str) = (
    "putio_transfers.transfer_failed",
    "Put.io ended this transfer with an error",
);

/// `CANCELLED` or `CANCELLING`: the transfer was stopped at Put.io, by somebody or something
/// that is not rDownloader.
pub const TRANSFER_CANCELLED: (&str, &str) = (
    "putio_transfers.transfer_cancelled",
    "This transfer was cancelled at Put.io",
);

/// The transfer is not in the account any more, or never was.
pub const TRANSFER_GONE: (&str, &str) = (
    "putio_transfers.transfer_gone",
    "Put.io no longer holds this transfer",
);

/// HTTP 401, or a 403 whose word says the token is the problem.
pub const AUTH_INVALID: (&str, &str) = (
    "putio_transfers.auth_invalid",
    "Put.io sign-in is invalid or expired",
);

/// HTTP 403: the credential is good and the account may not do this.
pub const NOT_PERMITTED: (&str, &str) = (
    "putio_transfers.not_permitted",
    "Put.io does not allow this account to do that",
);

/// The account has no room left for what this transfer would store.
pub const DISK_FULL: (&str, &str) = (
    "putio_transfers.disk_full",
    "The Put.io account has no space left for this transfer",
);

/// HTTP 429: the request budget is spent.
pub const RATE_LIMITED: (&str, &str) = (
    "putio_transfers.rate_limited",
    "Put.io API rate limit reached",
);

/// HTTP 5xx.
pub const SERVER_ERROR: (&str, &str) = (
    "putio_transfers.server_error",
    "Put.io is temporarily unavailable",
);

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = (
    "putio_transfers.invalid_response",
    "Invalid Put.io response",
);

/// The submit succeeded and Put.io named no transfer. Without an identifier there is nothing
/// to poll, nothing to hand over and nothing to delete.
pub const NO_TRANSFER_ID: (&str, &str) = (
    "putio_transfers.no_transfer_id",
    "Put.io did not name the transfer it created",
);

/// The transfer finished and names no folder or file at all.
pub const NO_CONTENT: (&str, &str) = (
    "putio_transfers.no_content",
    "Put.io reports this transfer as finished but names no files",
);

/// The finished transfer holds more files, or more folders, than one call may walk.
pub const TOO_MANY_FILES: (&str, &str) = (
    "putio_transfers.too_many_files",
    "This Put.io transfer holds more files than rDownloader will take in one job",
);

/// A selection. Put.io offers none, so this is never reached from the host — `poll` never
/// answers `awaiting-choice` — and it says so rather than silently doing something else.
pub const NO_REMOTE_SELECTION: (&str, &str) = (
    "putio_transfers.no_remote_selection",
    "Put.io downloads a whole torrent and offers no file selection before it does",
);

/// A refusal this build has no bucket for. The stable word travels as `reason`; Put.io's
/// sentence does not.
pub const API_ERROR: (&str, &str) = ("putio_transfers.api_error", "Put.io API error");

/// An HTTP status no error document explains.
pub const HTTP_ERROR: (&str, &str) = ("putio_transfers.http_error", "Put.io HTTP status");

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("Put.io HTTP status {status}")
}

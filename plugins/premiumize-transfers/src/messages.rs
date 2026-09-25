//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! **Nothing Premiumize wrote appears in any of them.** The API answers a refusal with
//! `{"status":"error","code":"<stable>","message":"<sentence>"}` and an HTTP `200`. The code
//! is stable and documented and travels as the `api_code` parameter; the sentence is neither
//! and is dropped. That is stricter than the resolver sibling, which still forwards `message`
//! as a parameter -- RD-120-23 asked for a stable code and a translation instead, and this is
//! where that begins.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The source is none of the three shapes this plugin takes.
pub const NOT_A_SOURCE: (&str, &str) = (
    "premiumize_transfers.not_a_source",
    "This is not a magnet address, a container or a usable link",
);

/// A container whose format the upload cannot name. `.ccf` has no marker of its own, so it
/// ends here rather than being uploaded under a guessed extension.
pub const CONTAINER_UNKNOWN: (&str, &str) = (
    "premiumize_transfers.container_unknown",
    "This container's format could not be recognised",
);

/// The call carried no account identity, so there is no credential it could run as.
pub const ACCOUNT_MISSING: (&str, &str) = (
    "premiumize_transfers.account_missing",
    "Premiumize account is missing",
);

/// The sign-in is gone: `authentication_failed`, or an HTTP 401/403 with no envelope.
pub const AUTH_INVALID: (&str, &str) = (
    "premiumize_transfers.auth_invalid",
    "Premiumize sign-in is invalid or expired",
);

/// `service_unsupported`: Premiumize will not take this source.
pub const SOURCE_UNSUPPORTED: (&str, &str) = (
    "premiumize_transfers.source_unsupported",
    "Premiumize does not accept this source",
);

/// `service_down`, `transient_error`, or a 5xx: the provider is busy right now.
pub const SERVER_BUSY: (&str, &str) = (
    "premiumize_transfers.server_busy",
    "Premiumize is temporarily unavailable",
);

/// `rate_limit_reached`, or HTTP 429.
pub const RATE_LIMITED: (&str, &str) = (
    "premiumize_transfers.rate_limited",
    "Premiumize API rate limit reached",
);

/// `fairuse_limit`, `account_limit_reached` and the rest of the quota family.
pub const LIMIT_REACHED: (&str, &str) = (
    "premiumize_transfers.limit_reached",
    "Premiumize reports the traffic or fair-use limit as reached",
);

/// `not_found`: the transfer is not in the account any more, or never was.
pub const TRANSFER_GONE: (&str, &str) = (
    "premiumize_transfers.transfer_gone",
    "Premiumize no longer holds this transfer",
);

/// `status = "error"` on a transfer: the provider ended it.
pub const TRANSFER_FAILED: (&str, &str) = (
    "premiumize_transfers.transfer_failed",
    "Premiumize ended this transfer with an error",
);

/// A `transfer/list` that does not name this job's transfer at all. Not the same as
/// `not_found` on a request: the account answered, and what it answered has no such row.
pub const TRANSFER_UNLISTED: (&str, &str) = (
    "premiumize_transfers.transfer_unlisted",
    "Premiumize does not list this transfer any more",
);

/// The submit succeeded and Premiumize named no transfer.
pub const NO_TRANSFER_ID: (&str, &str) = (
    "premiumize_transfers.no_transfer_id",
    "Premiumize did not name the transfer it created",
);

/// A finished transfer that carries neither a file nor a folder to read.
pub const NO_LOCATION: (&str, &str) = (
    "premiumize_transfers.no_location",
    "Premiumize reports this transfer as finished but named no file or folder",
);

/// A finished transfer whose folder holds nothing the LinkGrabber could take.
pub const NO_FILES: (&str, &str) = (
    "premiumize_transfers.no_files",
    "Premiumize reports this transfer as finished but returned no files",
);

/// A choice for a job that never asked one. `poll` here never answers `awaiting-choice`,
/// because Premiumize has no way to be told which files of a transfer to keep -- see the
/// header of `src/guest.rs`. Kept because a plugin that silently accepted an answer nobody
/// asked for would be reporting a selection that changed nothing.
pub const NO_CHOICE: (&str, &str) = (
    "premiumize_transfers.no_choice",
    "Premiumize transfers do not ask which files to take",
);

/// A finished transfer whose folder is deeper or wider than one poll may walk. Refused
/// rather than answered with a truncated list: a package missing half its files, with
/// nothing to say so, is the shape of the defect ADR 0001 was opened for.
pub const FOLDER_TOO_LARGE: (&str, &str) = (
    "premiumize_transfers.folder_too_large",
    "This transfer produced more folders than one poll can read",
);

/// A selection naming nothing. The host refuses one before it gets here.
pub const EMPTY_CHOICE: (&str, &str) = (
    "premiumize_transfers.empty_choice",
    "A file selection has to name at least one file",
);

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = (
    "premiumize_transfers.invalid_response",
    "Invalid Premiumize response",
);

/// A documented `code` this build has no bucket for. The code travels; the sentence does not.
pub const API_ERROR: (&str, &str) = ("premiumize_transfers.api_error", "Premiumize API error");

/// An HTTP status no envelope explains.
pub const HTTP_ERROR: (&str, &str) = ("premiumize_transfers.http_error", "Premiumize HTTP status");

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("Premiumize HTTP status {status}")
}

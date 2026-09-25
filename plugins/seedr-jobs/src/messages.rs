//! User-facing texts and stable failure codes.
//!
//! Every code here exists in all four shipped catalogues under `locales/`. The English text
//! beside it is the last rung, for a build whose catalogue does not know the code yet — it is
//! never a second channel, and nothing Seedr wrote is ever repeated in it.

/// The source is neither a magnet naming a BitTorrent info hash nor a readable `.torrent`.
pub const NOT_A_TORRENT: (&str, &str) = (
    "seedr_jobs.not_a_torrent",
    "Seedr takes a magnet address or a torrent file, and this is neither",
);

/// An ordinary web address. Seedr documents `POST /rest/transfer/url`, and its own heading for
/// it sits in the Transfers section beside the magnet and the torrent file, so it is a torrent
/// source rather than a general downloader. Claiming every http address for it would take them
/// away from the hosters that can actually fetch them.
pub const ADDRESS_UNSUPPORTED: (&str, &str) = (
    "seedr_jobs.address_unsupported",
    "Seedr transfers take a magnet address or a torrent file, not a web address",
);

/// Seedr answered the submit without naming the transfer it created.
pub const NO_TRANSFER_ID: (&str, &str) = (
    "seedr_jobs.no_transfer_id",
    "Seedr did not name the transfer it created",
);

/// The transfer is not in the account any more and left no folder behind.
pub const TRANSFER_GONE: (&str, &str) = (
    "seedr_jobs.transfer_gone",
    "Seedr no longer holds this transfer",
);

/// The transfer finished and its folder holds nothing that can be downloaded.
pub const NO_FILES: (&str, &str) = (
    "seedr_jobs.no_files",
    "Seedr reports this transfer as finished but its folder holds no files",
);

/// Seedr fetches whole torrents and offers no call that means "these files and not the
/// others", so nothing here ever asks. Never reached from the host, which only calls `choose`
/// after `awaiting-choice`.
pub const NO_SELECTION: (&str, &str) = (
    "seedr_jobs.no_selection",
    "Seedr downloads the whole transfer and has no file selection",
);

/// HTTP 401 or 403: Seedr refused the e-mail address and password.
pub const AUTH_INVALID: (&str, &str) = (
    "seedr_jobs.auth_invalid",
    "Seedr rejected this account's e-mail address or password",
);

/// HTTP 402, or a word naming the plan. Seedr's own documentation makes the REST API a
/// premium feature.
pub const PLAN_REQUIRED: (&str, &str) = (
    "seedr_jobs.plan_required",
    "The Seedr REST API needs a premium plan on this account",
);

/// The account has no room left. Seedr says so in its own answer and remembers the content, so
/// this waits rather than ending the job.
pub const OUT_OF_SPACE: (&str, &str) = (
    "seedr_jobs.out_of_space",
    "The Seedr account has no space left for this transfer",
);

/// HTTP 429.
pub const RATE_LIMITED: (&str, &str) = (
    "seedr_jobs.rate_limited",
    "The Seedr request limit was reached",
);

/// HTTP 5xx.
pub const SERVER_ERROR: (&str, &str) = ("seedr_jobs.server_error", "Seedr server error");

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) =
    ("seedr_jobs.invalid_response", "Invalid Seedr response");

/// A refusal this build has no bucket for. The code-shaped word travels as `reason`; Seedr's
/// prose never does.
pub const API_ERROR: (&str, &str) = ("seedr_jobs.api_error", "Seedr API error");

/// An HTTP status nothing in the answer explains.
pub const HTTP_ERROR: (&str, &str) = ("seedr_jobs.http_error", "Seedr HTTP status");

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("Seedr HTTP status {status}")
}

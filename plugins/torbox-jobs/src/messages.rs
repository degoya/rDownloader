//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! Nothing TorBox wrote appears in any of them. The API answers a refusal with
//! `{"success": false, "error": "<WORD>", "detail": "<sentence>"}`; the word is stable and
//! documented and travels as the `api_code` parameter, the sentence is not and is dropped.
//! The same rule the Real-Debrid siblings arrived at, for the same reason: `detail` has echoed
//! a submitted link back, and a message nobody has read cannot be promised not to.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The source is none of TorBox's three: not a magnet naming an info hash, not a readable
/// `.torrent` or `.nzb`, not an `http(s)` address.
pub const NOT_A_JOB: (&str, &str) = (
    "torbox_jobs.not_a_job",
    "This is not a magnet, a torrent file, an NZB or a web address",
);

/// `error`, and any other end TorBox reports without a reason of its own.
pub const JOB_FAILED: (&str, &str) = (
    "torbox_jobs.job_failed",
    "TorBox ended this job with an error",
);

/// `missingFiles`: TorBox finished and some of what it fetched is not there.
pub const JOB_INCOMPLETE: (&str, &str) = (
    "torbox_jobs.job_incomplete",
    "TorBox could not fetch every file of this job",
);

/// `unavailable`: TorBox will not carry this job.
pub const JOB_UNAVAILABLE: (&str, &str) = (
    "torbox_jobs.job_unavailable",
    "TorBox reports this job as unavailable",
);

/// The job is not in the account any more, or never was.
pub const JOB_GONE: (&str, &str) = ("torbox_jobs.job_gone", "TorBox no longer holds this job");

/// `DUPLICATE_ITEM`: the account already has this job. Not shown on the adoption path, which
/// turns it into a handle; kept because a submit that races itself can still end here.
pub const JOB_EXISTS: (&str, &str) = (
    "torbox_jobs.job_exists",
    "TorBox already holds a job for this content",
);

/// `LINK_OFFLINE`: the source itself is gone, not the job.
pub const SOURCE_GONE: (&str, &str) = (
    "torbox_jobs.source_gone",
    "TorBox could not reach the source of this job",
);

/// `BAD_TOKEN`, `AUTH_ERROR`, `NO_AUTH`, or HTTP 401/403 with nothing else to read.
pub const AUTH_INVALID: (&str, &str) = (
    "torbox_jobs.auth_invalid",
    "The TorBox API key is invalid or expired",
);

/// The account holds no TorBox API key at all.
pub const TOKEN_MISSING: (&str, &str) = (
    "torbox_jobs.token_missing",
    "This TorBox account has no API key",
);

/// The call carried no account identity, so there is no credential it could run as.
pub const ACCOUNT_MISSING: (&str, &str) =
    ("torbox_jobs.account_missing", "TorBox account is missing");

/// `PLAN_RESTRICTED_FEATURE`: this plan does not cover this kind of job.
pub const NOT_PERMITTED: (&str, &str) = (
    "torbox_jobs.not_permitted",
    "The TorBox plan does not cover this kind of job",
);

/// `DOWNLOAD_TOO_LARGE`, `TOO_MUCH_DATA`.
pub const TOO_LARGE: (&str, &str) = (
    "torbox_jobs.too_large",
    "TorBox refuses this job as too large",
);

/// A 5xx, `DATABASE_ERROR`, `DOWNLOAD_SERVER_ERROR`, `NO_SERVERS_AVAILABLE_ERROR`.
pub const SERVER_BUSY: (&str, &str) = (
    "torbox_jobs.server_busy",
    "TorBox is temporarily unavailable",
);

/// A 5xx with nothing else to read.
pub const SERVER_ERROR: (&str, &str) = ("torbox_jobs.server_error", "TorBox server error");

/// `MONTHLY_LIMIT`, `ACTIVE_LIMIT`, `DOWNLOAD_LIMIT`.
pub const LIMIT_REACHED: (&str, &str) = (
    "torbox_jobs.limit_reached",
    "TorBox reports the plan limit as reached",
);

/// `COOLDOWN_LIMIT`: too many jobs too quickly, and TorBox is making this account wait.
pub const COOLDOWN: (&str, &str) = (
    "torbox_jobs.cooldown",
    "TorBox is holding this account in a cooldown",
);

/// HTTP 429. Refused requests count towards the very cap that refused them, so this is a wait
/// with a floor rather than an immediate retry.
pub const RATE_LIMITED: (&str, &str) =
    ("torbox_jobs.rate_limited", "TorBox API rate limit reached");

/// `INVALID_OPTION` and its neighbours, HTTP 451: TorBox refused the request itself.
pub const REQUEST_REFUSED: (&str, &str) =
    ("torbox_jobs.request_refused", "TorBox refused this request");

/// TorBox performs no file selection. Never reached from the host, which only calls `choose`
/// after a job answered `awaiting-choice`, and this plugin never does; kept because a world is
/// all or nothing and a silent success here would be a promise nothing keeps.
pub const NO_SELECTION: (&str, &str) = (
    "torbox_jobs.no_selection",
    "TorBox downloads whole jobs and offers no file selection",
);

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) =
    ("torbox_jobs.invalid_response", "Invalid TorBox response");

/// The submit succeeded and TorBox named no job. Without an identifier there is nothing to
/// poll, nothing to fetch from and nothing to delete.
pub const NO_JOB_ID: (&str, &str) = (
    "torbox_jobs.no_job_id",
    "TorBox did not name the job it created",
);

/// The job finished and offers no file at all.
pub const NO_FILES: (&str, &str) = (
    "torbox_jobs.no_files",
    "TorBox reports this job as finished but lists no files",
);

/// The host's random source answered short. A boundary this plugin derived itself would be one
/// anybody could recompute, and a container carrying it would be split in the middle.
pub const NO_ENTROPY: (&str, &str) = (
    "torbox_jobs.no_entropy",
    "The host could not supply the randomness this request needs",
);

/// A documented `error` word this build has no bucket for. The word travels; TorBox's sentence
/// does not.
pub const API_ERROR: (&str, &str) = ("torbox_jobs.api_error", "TorBox API error");

/// An HTTP status no `error` word explains.
pub const HTTP_ERROR: (&str, &str) = ("torbox_jobs.http_error", "TorBox HTTP status");

#[must_use]
pub fn api_error(api_code: &str) -> String {
    format!("TorBox API error {api_code}")
}

#[must_use]
pub fn http_error(status: u16) -> String {
    format!("TorBox HTTP status {status}")
}

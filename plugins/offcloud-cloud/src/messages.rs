//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/{de,en,es,fr}.json` translate
//! exactly these codes and no others.
//!
//! Nothing Offcloud wrote appears in any of them. The API answers a refusal with
//! `{"error": "<sentence>"}` and the sentence is prose — prose that may quote the address it
//! was asked about. Only the one stable word the provider's own clients branch on (`NOAUTH`)
//! and the closed set of `not_available` reasons travel, and they travel as parameters of a
//! code rather than as text. The same rule the resolver sibling follows.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The source is neither a magnet naming a BitTorrent info hash nor an http(s) address.
pub const NOT_A_CLOUD_SOURCE: (&str, &str) = (
    "offcloud_cloud.not_a_cloud_source",
    "Offcloud takes a magnet address or a web address, and this is neither",
);

/// A `.torrent` or `.nzb` handed over as bytes. Offcloud's cloud takes one field and it is an
/// address, so there is nowhere for a container to go.
pub const CONTAINER_UNSUPPORTED: (&str, &str) = (
    "offcloud_cloud.container_unsupported",
    "Offcloud takes an address, not a torrent or NZB file",
);

/// `NOAUTH`, or HTTP 401/403 with nothing else to read.
pub const AUTH_INVALID: (&str, &str) = (
    "offcloud_cloud.auth_invalid",
    "The Offcloud API key is invalid or no longer valid",
);

/// `not_available`: this job needs an add-on the account has not bought.
pub const ADDON_REQUIRED: (&str, &str) = (
    "offcloud_cloud.addon_required",
    "Offcloud needs an additional add-on for this download",
);

/// The job is not in the account any more, or never was.
pub const JOB_GONE: (&str, &str) = (
    "offcloud_cloud.job_gone",
    "Offcloud no longer holds this cloud download",
);

/// Offcloud reported the job as ended with an error of its own.
pub const JOB_FAILED: (&str, &str) = (
    "offcloud_cloud.job_failed",
    "Offcloud ended this cloud download with an error",
);

/// Offcloud reported the job as cancelled at the provider.
pub const JOB_CANCELED: (&str, &str) = (
    "offcloud_cloud.job_canceled",
    "This cloud download was cancelled at Offcloud",
);

/// The submit succeeded and Offcloud named no request. Without an identifier there is nothing
/// to poll, nothing to explore and nothing to remove.
pub const NO_REQUEST_ID: (&str, &str) = (
    "offcloud_cloud.no_request_id",
    "Offcloud did not name the cloud download it created",
);

/// The job finished and carries no address at all.
pub const NO_LINKS: (&str, &str) = (
    "offcloud_cloud.no_links",
    "Offcloud reports this cloud download as finished but returned no addresses",
);

/// Offcloud's cloud fetches the whole of what it was given, so nothing ever waits for a
/// selection. Never reached from the host, which only calls `choose` after `awaiting-choice`.
pub const NO_SELECTION: (&str, &str) = (
    "offcloud_cloud.no_selection",
    "Offcloud downloads the whole job and has no file selection",
);

/// HTTP 429, or an exhausted allowance.
pub const RATE_LIMITED: (&str, &str) = (
    "offcloud_cloud.rate_limited",
    "The Offcloud request limit was reached",
);

/// A 5xx with nothing else to read.
pub const SERVER_ERROR: (&str, &str) = ("offcloud_cloud.server_error", "Offcloud server error");

/// The API answered with something that is not the expected JSON.
pub const INVALID_RESPONSE: (&str, &str) = (
    "offcloud_cloud.invalid_response",
    "Invalid Offcloud response",
);

/// A refusal this build has no bucket for. The provider's word travels as `api_code` when it
/// is code-shaped; its prose never does.
pub const API_ERROR: (&str, &str) = ("offcloud_cloud.api_error", "Offcloud API error");

/// An HTTP status nothing in the answer explains.
pub const HTTP_ERROR: (&str, &str) = ("offcloud_cloud.http_error", "Offcloud HTTP status");

//! Target-independent TorBox logic: the envelope every endpoint answers in, the three sets of
//! paths, the request bodies, the state machine over TorBox's own words, and the failure
//! classification.
//!
//! Written against the published API document at <https://api.torbox.app/>:
//!
//! - **Auth flavour**: `Authorization: Bearer <API key>`, the key the person pasted into the
//!   account. This plugin never sees it: every request carries the template
//!   `{{secret:torbox_api_key}}` and the host expands it towards `api.torbox.app` and nowhere
//!   else.
//! - **Envelope**: every answer is `{"success": bool, "error": <string|null>, "detail":
//!   "<sentence>", "data": <the answer>}`. `error` is a stable upper-case word and travels;
//!   `detail` is prose TorBox wrote and is dropped, exactly as the Real-Debrid sibling drops
//!   its `error` sentence. A `success: false` inside an HTTP 200 is still a refusal.
//! - **Submit**: `POST /{torrents/createtorrent, usenet/createusenetdownload,
//!   webdl/createwebdownload}` as `multipart/form-data`. **None of them is idempotent** --
//!   each call creates another job in the account -- which is the single fact the whole design
//!   in `docs/adr/0003-a-job-that-runs-at-the-provider.md` is built around.
//! - **Adopt**: `GET /{kind}/mylist` lists what the account already holds, each entry carrying
//!   the `hash` this plugin derives locally. That is what closes the window between a submit
//!   going out and its answer coming back.
//! - **Poll**: `GET /{kind}/mylist?id={id}&bypass_cache=true` carries `download_state`,
//!   `progress`, `files`, `download_finished` and `download_present`.
//! - **Finish**: `GET /{kind}/requestdl?token=<key>&{torrent,usenet,web}_id={id}&file_id={fid}`
//!   mints a short-lived address. This plugin hands back the `requestdl` address **without the
//!   token**, because it has no token to put there and because a stable address is what the
//!   queue can re-resolve; `plugins/torbox/` adds the key and mints the ticket, on every
//!   attempt.
//! - **Discard**: `POST /{kind}/control...`, and only ever from a confirmed request.
//! - **Rate limit**: TorBox answers 429 with `Retry-After`. The suggested waits below are
//!   generous rather than eager, because the polling shares the account's budget with the
//!   resolver minting this very job's addresses.

use plugin_common::failure::{ApiFailure, ErrorKind, HttpError, HttpWords};
use serde::Deserialize;
use torbox_common::Words;
pub use torbox_common::{ErrorEnvelope, is_safe_id as is_safe_remote_id};

use crate::messages;

mod bodies;
mod cache;
mod paths;

pub use bodies::{boundary, control_body, multipart, multipart_content_type};
pub use cache::{CachedEntry, cached_entries};
pub use paths::{
    check_cached_path, container_name, control_id_field, control_path, create_path,
    download_address, list_path, request_id_field, request_path, text_field,
};

/// The vault reference the TorBox provider keeps its API key under. The value never reaches
/// this plugin.
pub const TOKEN_REFERENCE: &str = "torbox_api_key";

pub const API_BASE: &str = "https://api.torbox.app/v1/api";

/// How many of the account's jobs `adopt` reads before giving up on finding the digest.
///
/// One page is enough for the case this exists for -- a submit that was lost seconds ago is
/// the newest entry there is -- and an adoption that does not find it falls through to the
/// host's attempt ceiling rather than paging for ever.
pub const ADOPT_LIMIT: u32 = 100;

/// Longest file list a single job contributes. A job with more entries than this is reported
/// truncated rather than refused: the host caps the artifacts again on its own side, and a
/// person with a thousand-file release would rather have the first hundreds than a failure.
pub const MAX_FILES: usize = 2000;

// --- Answer shapes ---------------------------------------------------------------------

/// `data` of a create call. TorBox spells the identifier differently per kind, so every
/// spelling is read and the first one present wins.
#[derive(Default, Deserialize)]
pub struct CreatedJob {
    #[serde(default)]
    pub torrent_id: Option<serde_json::Value>,
    #[serde(default)]
    pub usenetdownload_id: Option<serde_json::Value>,
    #[serde(default)]
    pub webdownload_id: Option<serde_json::Value>,
    #[serde(default)]
    pub hash: Option<String>,
    /// Present when TorBox parked the job instead of starting it. It is not a job identifier
    /// and is deliberately not used as one.
    #[serde(default)]
    pub queued_id: Option<serde_json::Value>,
}

impl CreatedJob {
    /// The identifier TorBox gave the job, as the string a handle carries.
    #[must_use]
    pub fn id(&self) -> Option<String> {
        [
            &self.torrent_id,
            &self.usenetdownload_id,
            &self.webdownload_id,
        ]
        .into_iter()
        .flatten()
        .find_map(identifier)
    }
}

/// One file inside a job.
#[derive(Default, Deserialize)]
pub struct JobFile {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    /// The path inside the job, rooted at the job's own name.
    #[serde(default)]
    pub name: Option<String>,
    /// The bare file name, when TorBox states one.
    #[serde(default)]
    pub short_name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// One entry of a `mylist` answer.
#[derive(Default, Deserialize)]
pub struct JobEntry {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    /// The digest TorBox knows the job by; compared case-insensitively with the content key,
    /// because the two come from different places and neither promises a case.
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub download_state: Option<String>,
    /// A fraction between zero and one, **not** a percentage.
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub download_speed: Option<u64>,
    #[serde(default)]
    pub eta: Option<u64>,
    #[serde(default)]
    pub download_finished: Option<bool>,
    #[serde(default)]
    pub download_present: Option<bool>,
    #[serde(default)]
    pub files: Vec<JobFile>,
}

impl JobEntry {
    /// The identifier, as the string a handle carries.
    #[must_use]
    pub fn identifier(&self) -> Option<String> {
        self.id.as_ref().and_then(identifier)
    }

    /// Whether the bytes are at TorBox and can be asked for.
    #[must_use]
    pub fn is_present(&self) -> bool {
        self.download_present.unwrap_or(false) && self.download_finished.unwrap_or(false)
    }
}

/// Reads an identifier TorBox states either as a number or as a string.
fn identifier(value: &serde_json::Value) -> Option<String> {
    let text = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.trim().to_owned(),
        _ => return None,
    };
    is_safe_remote_id(&text).then_some(text)
}

// --- The state machine -----------------------------------------------------------------

/// Where a job stands, in the vocabulary of `interface remote-job` rather than of TorBox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Doing something that needs nobody, with a suggested wait in seconds.
    Preparing(u64),
    /// Fetching, with a suggested wait.
    Working(u64),
    /// Finished, and the bytes are at TorBox.
    Ready,
    /// TorBox ended it, with the code and text that say how.
    Failed((&'static str, &'static str)),
}

/// Maps one of TorBox's `download_state` words.
///
/// Four of the choices are worth stating rather than reading out of the table:
///
/// - **`download_present` decides, not the word.** TorBox reports `completed`, `cached` and
///   `uploading` for jobs whose bytes are already servable and for jobs whose bytes are not
///   yet, so the two booleans it states beside the word are believed before the word is. A
///   job that says `completed` and has nothing to fetch is `Preparing`, not `Ready` -- and a
///   `Ready` with nothing behind it would hand the LinkGrabber an empty package.
/// - **`cached` is not a state of its own here.** It means TorBox served the job out of its
///   own cache instead of fetching it, which is a fact about how fast it went and not about
///   where it stands. It reaches `Ready` by the same rule as everything else, which is also
///   why nothing in this plugin presents a cache hit as a promise.
/// - **`stalled` is `Working` and not a failure.** A torrent with no seeds right now may have
///   seeds in ten minutes, and a plugin that failed the job would throw away one that was
///   about to start.
/// - **An unknown word is `Preparing`.** TorBox has added states before, and failing on a word
///   this build has not heard of would end jobs that are going perfectly well.
#[must_use]
pub fn stage_of(entry: &JobEntry) -> Stage {
    let state = entry
        .download_state
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if is_failed_state(&state) {
        return Stage::Failed(failure_of(&state));
    }
    if entry.is_present() {
        return Stage::Ready;
    }
    if state.contains("stall") {
        // Waiting on somebody else, and the answer to that is not to ask faster.
        return Stage::Working(60);
    }
    match state.as_str() {
        "downloading" | "uploading" => Stage::Working(30),
        "metadl" | "checkingresumedata" | "processing" | "repairing" | "verifying" => {
            Stage::Preparing(15)
        }
        "paused" => Stage::Preparing(120),
        "queued" | "inqueue" | "" => Stage::Preparing(30),
        // `completed` and `cached` arrive here only while the bytes are not servable yet,
        // which is the short window between TorBox finishing and TorBox publishing.
        "completed" | "cached" => Stage::Preparing(10),
        _ => Stage::Preparing(60),
    }
}

fn is_failed_state(state: &str) -> bool {
    matches!(state, "error" | "missingfiles" | "failed" | "unavailable")
}

fn failure_of(state: &str) -> (&'static str, &'static str) {
    match state {
        "missingfiles" => messages::JOB_INCOMPLETE,
        "unavailable" => messages::JOB_UNAVAILABLE,
        _ => messages::JOB_FAILED,
    }
}

/// TorBox's progress fraction, in the thousandths the contract carries.
///
/// A fraction and not a percentage: TorBox states `0.425`, Real-Debrid states `42.5`, and
/// reading one as the other is the difference between a bar at 42 % and a bar that never
/// leaves zero.
#[must_use]
pub fn permille(progress: Option<f64>) -> Option<u16> {
    let progress = progress?;
    if !progress.is_finite() {
        return None;
    }
    let scaled = (progress * 1_000.0).round().clamp(0.0, 1_000.0);
    // The clamp above bounds the value into u16 range before the cast, so nothing is lost.
    Some(scaled as u16)
}

/// Where one finished file belongs: its bare name, and the path it sat on inside the job.
///
/// The job's own name is the root of that path, which is what turns "the magnet I pasted" into
/// the package a person expects. TorBox already roots every path of a multi-file job at that
/// name (`Show.S01/ep01.mkv` under a job called `Show.S01`), so a leading segment that *is*
/// the name is not repeated: without this every package a remote job produced would land in a
/// `Show.S01/Show.S01` folder.
#[must_use]
pub fn place(job_name: &str, path: &str) -> (Option<String>, Option<String>) {
    let mut segments: Vec<&str> = path
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect();
    let file_name = segments.pop().map(str::to_owned);
    let mut place = Vec::new();
    let job_name = job_name.trim();
    if !job_name.is_empty() {
        place.push(job_name);
        if segments.first().is_some_and(|first| *first == job_name) {
            segments.remove(0);
        }
    }
    place.extend(segments);
    let hint = (!place.is_empty()).then(|| place.join("/"));
    (file_name, hint)
}

// --- Failures --------------------------------------------------------------------------

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// multihoster plugins settled on.
const BUSY_SECONDS: u64 = 300;

/// How long an exhausted quota is waited out.
const QUOTA_SECONDS: u64 = 3600;

/// How long a cooldown is waited out. TorBox states no figure, and its cooldown is measured in
/// minutes rather than hours.
const COOLDOWN_SECONDS: u64 = 600;

/// How TorBox's codes name an HTTP status no document in the answer explains: the classes
/// are `plugin_common::http_status`'s, the one mapping every plugin shares (RD-191-07); a `429`
/// or a `5xx` carries the response's `Retry-After` into the wait.
///
/// A `404`/`410` is the job gone for good; a legal block (`451`) is `Offline` there and retried,
/// still worded `REQUEST_REFUSED` like the words that say TorBox refused the request itself
/// (RA-PLG-04). A `429` without a stated wait waits a minute, a `5xx` five minutes.
pub const HTTP: HttpWords = HttpWords {
    unauthorized: messages::AUTH_INVALID,
    gone: messages::JOB_GONE,
    unavailable: messages::REQUEST_REFUSED,
    rate_limited: messages::RATE_LIMITED,
    server_error: messages::SERVER_ERROR,
    rate_limited_wait: Some(60),
    server_error_wait: Some(BUSY_SECONDS),
    other: HttpError {
        code: messages::HTTP_ERROR.0,
        text: messages::http_error,
    },
};

/// Classifies one of TorBox's documented `error` words.
///
/// The word travels as the `api_code` parameter and the sentence beside it does not: the word
/// is stable and documented, `detail` is prose that changes and that has echoed a submitted
/// link before now.
#[must_use]
pub fn classify_error(api_code: &str, retry_after: Option<u64>) -> ApiFailure {
    match api_code {
        "BAD_TOKEN" | "AUTH_ERROR" | "NO_AUTH" | "OAUTH_VERIFICATION_ERROR" => {
            ApiFailure::with_api_code(ErrorKind::AccountInvalid, messages::AUTH_INVALID, api_code)
        }
        "PLAN_RESTRICTED_FEATURE" => {
            ApiFailure::with_api_code(ErrorKind::Unsupported, messages::NOT_PERMITTED, api_code)
        }
        // TorBox already holds this job. Reported as permanent so the host stops submitting:
        // asking again only creates another one, and the adoption check is what turns this
        // into a handle. `JOB_EXISTS` is what tells it apart from a refusal.
        "DUPLICATE_ITEM" => {
            ApiFailure::with_api_code(ErrorKind::Permanent, messages::JOB_EXISTS, api_code)
        }
        "ITEM_NOT_FOUND" | "ENDPOINT_NOT_FOUND" => {
            ApiFailure::with_api_code(ErrorKind::Offline, messages::JOB_GONE, api_code)
        }
        "LINK_OFFLINE" | "BOZO_RSS_FEED" => {
            ApiFailure::with_api_code(ErrorKind::Offline, messages::SOURCE_GONE, api_code)
        }
        "DOWNLOAD_TOO_LARGE" | "TOO_MUCH_DATA" => {
            ApiFailure::with_api_code(ErrorKind::Permanent, messages::TOO_LARGE, api_code)
        }
        // TorBox's own figure wins where it stated one: a word says which bucket a refusal is
        // in, a `Retry-After` says when the provider is ready, and guessing over an answer is
        // how a wait ends up either pointless or twice as long as it had to be.
        "MONTHLY_LIMIT" | "ACTIVE_LIMIT" | "DOWNLOAD_LIMIT" => ApiFailure::with_api_code(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(QUOTA_SECONDS))),
            messages::LIMIT_REACHED,
            api_code,
        ),
        "COOLDOWN_LIMIT" => ApiFailure::with_api_code(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(COOLDOWN_SECONDS))),
            messages::COOLDOWN,
            api_code,
        ),
        "TOO_MANY_REQUESTS" => ApiFailure::with_api_code(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
            api_code,
        ),
        "DATABASE_ERROR"
        | "DOWNLOAD_SERVER_ERROR"
        | "NO_SERVERS_AVAILABLE_ERROR"
        | "VENDOR_ERROR"
        | "VENDOR_DISABLED" => ApiFailure::with_api_code(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_BUSY,
            api_code,
        ),
        "INVALID_OPTION" | "MISSING_REQUIRED_OPTION" | "TOO_MANY_OPTIONS" | "INVALID_DEVICE" => {
            ApiFailure::with_api_code(ErrorKind::Permanent, messages::REQUEST_REFUSED, api_code)
        }
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR.0,
            message: messages::api_error(other),
            params: vec![("api_code", other.to_owned())],
        },
    }
}

/// The words this plugin reports a refusal under; the order they are believed in is
/// `torbox_common`'s, shared with the resolver `plugins/torbox/`.
pub const WORDS: Words = Words {
    http: HTTP,
    classify: classify_error,
    api_error: messages::API_ERROR.0,
    refused: messages::REQUEST_REFUSED,
};

/// The failure an answer describes, or `None` when it describes none.
///
/// An answer is a failure when it names an `error`, whatever its HTTP status; a 200 carrying
/// one is still a refusal, and a 4xx carrying none is classified by its status alone.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    torbox_common::failure_from(status, retry_after, envelope, &WORDS)
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

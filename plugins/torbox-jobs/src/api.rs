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

use serde::Deserialize;

use crate::{messages, source::Kind};

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

// --- Paths -----------------------------------------------------------------------------

/// `POST` here to create a job of this kind.
#[must_use]
pub const fn create_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/createtorrent",
        Kind::Usenet => "/usenet/createusenetdownload",
        Kind::Web => "/webdl/createwebdownload",
    }
}

/// `GET` here to read one job of this kind, or the account's list of them.
#[must_use]
pub const fn list_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/mylist",
        Kind::Usenet => "/usenet/mylist",
        Kind::Web => "/webdl/mylist",
    }
}

/// `GET` here to mint a download address for one file of this kind of job.
#[must_use]
pub const fn request_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/requestdl",
        Kind::Usenet => "/usenet/requestdl",
        Kind::Web => "/webdl/requestdl",
    }
}

/// `POST` here to delete a job of this kind.
#[must_use]
pub const fn control_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/controltorrent",
        Kind::Usenet => "/usenet/controlusenetdownload",
        Kind::Web => "/webdl/controlwebdownload",
    }
}

/// `GET` here to ask whether TorBox holds content of this kind ready (RD-130-11).
#[must_use]
pub const fn check_cached_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/checkcached",
        Kind::Usenet => "/usenet/checkcached",
        Kind::Web => "/webdl/checkcached",
    }
}

/// The query parameter `requestdl` names the job by.
#[must_use]
pub const fn request_id_field(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "torrent_id",
        Kind::Usenet => "usenet_id",
        Kind::Web => "web_id",
    }
}

/// The field the control endpoint names the job by.
///
/// Deliberately its own function rather than [`request_id_field`]: TorBox spells the web
/// download's identifier `web_id` when it mints an address and `webdl_id` when it deletes one,
/// and a single spelling would be wrong at one of the two ends.
#[must_use]
pub const fn control_id_field(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "torrent_id",
        Kind::Usenet => "usenet_id",
        Kind::Web => "webdl_id",
    }
}

/// The multipart field a source of this kind is submitted under, when it is submitted as text.
#[must_use]
pub const fn text_field(kind: Kind) -> &'static str {
    match kind {
        // A magnet; a container of this kind goes in as a file instead.
        Kind::Torrent => "magnet",
        Kind::Usenet => "link",
        Kind::Web => "link",
    }
}

/// The file name a container of this kind is submitted under.
///
/// TorBox reads the bytes, not the name, but a multipart part has to carry one and a name that
/// says what the part is beats a generic one in anybody's server log.
#[must_use]
pub const fn container_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "upload.torrent",
        Kind::Usenet => "upload.nzb",
        Kind::Web => "upload.bin",
    }
}

/// The stable address one finished file is fetched from.
///
/// **Without the token**, deliberately. `requestdl` needs the account's API key as a query
/// parameter and this plugin has none: it names secrets, it never holds them. So what travels
/// is the address that identifies the file and nothing else, `plugins/torbox/` claims it, and
/// the key is added by the host on every resolve -- which is also what makes the short-lived
/// ticket behind it renewable rather than a one-shot value written into a row.
#[must_use]
pub fn download_address(kind: Kind, remote_id: &str, file_id: u32) -> String {
    format!(
        "{API_BASE}{}?{}={remote_id}&file_id={file_id}",
        request_path(kind),
        request_id_field(kind)
    )
}

// --- Answer shapes ---------------------------------------------------------------------

/// The envelope every endpoint answers in, read for its failure half alone.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub success: Option<bool>,
    /// TorBox's stable upper-case word. Typed as a free value because the field is `null` on
    /// success, `false` at one or two endpoints, and a string when it means something.
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    // `detail` is deliberately not read: it is prose TorBox wrote, and nothing here forwards
    // a provider's sentence.
}

impl ErrorEnvelope {
    /// The error word, upper-cased, or `None` when the answer names none.
    #[must_use]
    pub fn code(&self) -> Option<String> {
        let text = self.error.as_ref()?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_ascii_uppercase())
    }
}

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

/// Whether a provider-supplied identifier is safe to put in a request.
///
/// It comes back from TorBox and goes out again in a query, so it is checked rather than
/// trusted: an identifier carrying a slash or an ampersand would be a request to somewhere
/// else on the very host this plugin is allowed to reach.
#[must_use]
pub fn is_safe_remote_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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

// --- Request bodies --------------------------------------------------------------------

/// The multipart boundary, derived from bytes the host's random source produced.
///
/// Random rather than fixed because a container is somebody else's file: a fixed boundary that
/// happened to occur inside an NZB would split the part in the middle and submit half a
/// document. An empty answer from the host is a refusal, not an invitation to invent one, so
/// the caller checks the length before this is reached.
#[must_use]
pub fn boundary(entropy: &[u8]) -> String {
    let mut text = String::from("rdownloader");
    for byte in entropy {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// One `multipart/form-data` body: text fields first, then at most one file part.
#[must_use]
pub fn multipart(
    boundary: &str,
    fields: &[(&str, &str)],
    file: Option<(&str, &str, &[u8])>,
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    if let Some((name, file_name, bytes)) = file {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// The `Content-Type` a [`multipart`] body is sent under.
#[must_use]
pub fn multipart_content_type(boundary: &str) -> String {
    format!("multipart/form-data; boundary={boundary}")
}

/// The JSON body a control endpoint takes.
#[must_use]
pub fn control_body(kind: Kind, remote_id: &str, operation: &str) -> Vec<u8> {
    // `remote_id` has passed `is_safe_remote_id`, so it carries no quote and no backslash and
    // needs no escaping; it is written through `serde_json` anyway rather than formatted, so
    // that the guarantee lives in one place instead of in every caller's head.
    let value = serde_json::json!({
        control_id_field(kind): remote_id,
        "operation": operation,
    });
    serde_json::to_vec(&value).unwrap_or_default()
}

// --- Failures --------------------------------------------------------------------------

/// How a refusal is classified, without depending on either failure representation.
#[derive(Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AccountInvalid,
    RateLimited(Option<u64>),
    Unsupported,
    /// TorBox already holds this job. Not a failure at all at the one call site that can act
    /// on it: the adoption path turns it into a handle.
    Duplicate,
}

/// A classified refusal.
#[derive(Debug)]
pub struct ApiFailure {
    pub kind: ErrorKind,
    pub code: &'static str,
    pub message: String,
    pub params: Vec<(&'static str, String)>,
}

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// multihoster plugins settled on.
const BUSY_SECONDS: u64 = 300;

/// How long an exhausted quota is waited out.
const QUOTA_SECONDS: u64 = 3600;

/// How long a cooldown is waited out. TorBox states no figure, and its cooldown is measured in
/// minutes rather than hours.
const COOLDOWN_SECONDS: u64 = 600;

fn coded(kind: ErrorKind, (code, message): (&'static str, &str), api_code: &str) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: vec![("api_code", api_code.to_owned())],
    }
}

fn plain(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// Classifies one of TorBox's documented `error` words.
///
/// The word travels as the `api_code` parameter and the sentence beside it does not: the word
/// is stable and documented, `detail` is prose that changes and that has echoed a submitted
/// link before now.
#[must_use]
pub fn classify_error(api_code: &str, retry_after: Option<u64>) -> ApiFailure {
    match api_code {
        "BAD_TOKEN" | "AUTH_ERROR" | "NO_AUTH" | "OAUTH_VERIFICATION_ERROR" => {
            coded(ErrorKind::AccountInvalid, messages::AUTH_INVALID, api_code)
        }
        "PLAN_RESTRICTED_FEATURE" => {
            coded(ErrorKind::Unsupported, messages::NOT_PERMITTED, api_code)
        }
        "DUPLICATE_ITEM" => coded(ErrorKind::Duplicate, messages::JOB_EXISTS, api_code),
        "ITEM_NOT_FOUND" | "ENDPOINT_NOT_FOUND" => {
            coded(ErrorKind::Offline, messages::JOB_GONE, api_code)
        }
        "LINK_OFFLINE" | "BOZO_RSS_FEED" => {
            coded(ErrorKind::Offline, messages::SOURCE_GONE, api_code)
        }
        "DOWNLOAD_TOO_LARGE" | "TOO_MUCH_DATA" => {
            coded(ErrorKind::Permanent, messages::TOO_LARGE, api_code)
        }
        // TorBox's own figure wins where it stated one: a word says which bucket a refusal is
        // in, a `Retry-After` says when the provider is ready, and guessing over an answer is
        // how a wait ends up either pointless or twice as long as it had to be.
        "MONTHLY_LIMIT" | "ACTIVE_LIMIT" | "DOWNLOAD_LIMIT" => coded(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(QUOTA_SECONDS))),
            messages::LIMIT_REACHED,
            api_code,
        ),
        "COOLDOWN_LIMIT" => coded(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(COOLDOWN_SECONDS))),
            messages::COOLDOWN,
            api_code,
        ),
        "TOO_MANY_REQUESTS" => coded(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
            api_code,
        ),
        "DATABASE_ERROR"
        | "DOWNLOAD_SERVER_ERROR"
        | "NO_SERVERS_AVAILABLE_ERROR"
        | "VENDOR_ERROR"
        | "VENDOR_DISABLED" => coded(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_BUSY,
            api_code,
        ),
        "INVALID_OPTION" | "MISSING_REQUIRED_OPTION" | "TOO_MANY_OPTIONS" | "INVALID_DEVICE" => {
            coded(ErrorKind::Permanent, messages::REQUEST_REFUSED, api_code)
        }
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR.0,
            message: messages::api_error(other),
            params: vec![("api_code", other.to_owned())],
        },
    }
}

/// The failure an answer describes, or `None` when it describes none.
///
/// An answer is a failure when it names an `error`, whatever its HTTP status; a 200 carrying
/// one is still a refusal, and a 4xx carrying none is classified by its status alone. Both
/// directions matter, because TorBox uses both.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if let Some(api_code) = envelope.code() {
        let classified = classify_error(&api_code, retry_after);
        // A word this build has no bucket for is not the end of what the answer said. When the
        // status carries a meaning of its own -- a 429, a 5xx, a 401 -- that meaning is better
        // than "permanent, unknown word", and it is the difference between a wait and a job
        // somebody has to start again by hand. The word still travels as the parameter.
        if classified.code == messages::API_ERROR.0
            && let Err(by_status) = ensure_http_status(status, retry_after)
        {
            return Some(ApiFailure {
                params: vec![("api_code", api_code)],
                ..by_status
            });
        }
        return Some(classified);
    }
    if envelope.success == Some(false) {
        // A refusal TorBox did not name. The status is tried first, because most of these
        // carry one that says something; a `success: false` inside a 200 says only that the
        // call did not do what it was asked, and that is permanent rather than worth a retry.
        return Some(
            ensure_http_status(status, retry_after)
                .err()
                .unwrap_or_else(|| plain(ErrorKind::Permanent, messages::REQUEST_REFUSED)),
        );
    }
    if !(200..=299).contains(&status) {
        return ensure_http_status(status, retry_after).err();
    }
    None
}

/// Maps an HTTP status no `error` word explains.
///
/// # Errors
///
/// The classified refusal, for every status that is not a 2xx.
pub fn ensure_http_status(status: u16, retry_after: Option<u64>) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(plain(ErrorKind::AccountInvalid, messages::AUTH_INVALID)),
        404 | 410 => Err(plain(ErrorKind::Offline, messages::JOB_GONE)),
        429 => Err(plain(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
        )),
        451 => Err(plain(ErrorKind::Permanent, messages::REQUEST_REFUSED)),
        500..=599 => Err(plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_ERROR,
        )),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR.0,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
    }
}

// --- Cache check (RD-130-11) ----------------------------------------------------------

/// One thing `checkcached` says TorBox holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedEntry {
    /// The digest it is held under, as TorBox spelled it. Compared without regard to case.
    pub hash: String,
    pub name: Option<String>,
    pub size: Option<u64>,
}

/// What a `checkcached` answer says is held, or `None` when the answer is not one.
///
/// Tolerant on purpose, because the shape is not pinned down anywhere: TorBox's OpenAPI
/// document leaves the response schema empty, and its SDK says `data` is a dictionary of
/// `{name, size, hash}` without saying what it is keyed by. So `data` is read as an object
/// keyed by hash, as a list of entries, or as a single entry; `null`, `false`, `{}` and `[]`
/// all mean "nothing held". Only an answer that is not JSON, or carries no `data` at all, is
/// refused. TorBox names only what it holds, so nothing here ever says "known but not held".
#[must_use]
pub fn cached_entries(body: &[u8]) -> Option<Vec<CachedEntry>> {
    let envelope: serde_json::Value = serde_json::from_slice(body).ok()?;
    let data = envelope.as_object()?.get("data")?;
    Some(match data {
        serde_json::Value::Object(map) => {
            if map.get("hash").is_some_and(serde_json::Value::is_string) {
                cached_entry(data, None).into_iter().collect()
            } else {
                map.iter()
                    .filter_map(|(key, value)| match value {
                        serde_json::Value::Object(_) => cached_entry(value, Some(key)),
                        serde_json::Value::Bool(true) => cached_entry(value, Some(key)),
                        _ => None,
                    })
                    .collect()
            }
        }
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_json::Value::String(hash) => cached_entry(item, Some(hash)),
                _ => cached_entry(item, None),
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// One entry, with the hash taken from the entry itself or, failing that, from its key.
fn cached_entry(value: &serde_json::Value, key: Option<&str>) -> Option<CachedEntry> {
    let hash = value
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .or(key)
        .map(str::trim)
        .filter(|hash| !hash.is_empty())?;
    Some(CachedEntry {
        hash: hash.to_owned(),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        size: value.get("size").and_then(cached_size),
    })
}

/// A size as TorBox states it: an integer, or a float in its SDK's model. Truncated; a
/// negative or non-finite one is no size at all.
// `as` saturates for a float above `u64::MAX`, and the checks rule out the two cases where it
// would invent a number.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn cached_size(value: &serde_json::Value) -> Option<u64> {
    if let Some(size) = value.as_u64() {
        return Some(size);
    }
    let size = value.as_f64()?;
    (size.is_finite() && size >= 0.0).then_some(size as u64)
}

/// Reads a `Retry-After` header stated in seconds. A date-shaped one is ignored rather than
/// guessed at: a wrong wait is worse than the bucket's own default.
#[must_use]
pub fn retry_after_seconds(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

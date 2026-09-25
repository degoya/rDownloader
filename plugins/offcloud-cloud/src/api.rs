//! Target-independent Offcloud `cloud/*` logic: response shapes, the state machine over the
//! provider's own words, and the failure classification.
//!
//! Written against the provider's own API documentation, <https://offcloud.com/api> and the
//! repository it points at, <https://github.com/Offcloud/offcloud-api>:
//!
//! - **Auth flavour**: `Authorization: Bearer <api key>`, the same key the resolver sibling
//!   uses. This plugin never sees it: every request carries the template
//!   `{{secret:offcloud_api_key}}` and the host expands it towards `offcloud.com` and nowhere
//!   else. The published document also offers `?key=<api key>`; the header is used instead
//!   because a query parameter ends up in every redirect chain and every log line that quotes
//!   an address.
//! - **Submit**: `POST /api/cloud` with a form body carrying `url`, which takes a magnet as
//!   readily as an ordinary address. It answers `{"requestId": "...", "fileName": "...",
//!   "status": "...", "originalLink": "..."}`. **It is not idempotent** — each call creates
//!   another request in the account — which is the single fact the design in
//!   `docs/adr/0003-a-job-that-runs-at-the-provider.md` is built around.
//! - **Adopt**: `GET /api/cloud/history` lists what the account already holds, each entry
//!   carrying the `originalLink` it was started from. That is what closes the window between a
//!   submit going out and its answer coming back.
//! - **Poll**: `POST /api/cloud/status` with `requestId`, answering `{"status": { ... }}` — a
//!   nested object, not a bare one.
//! - **Explore**: `GET /api/cloud/explore/{requestId}`, the file tree of a finished job. This
//!   is where the folder structure and the file names come from, and it is the reason a
//!   finished job costs two requests rather than one.
//! - **Discard**: `POST /api/cloud/remove`, and only ever from a confirmed request.
//! - **Refusals**: `{"error": "<sentence>"}`, and `{"not_available": "<reason>"}` when the
//!   account would need an add-on. `NOAUTH` is the one stable word in the first shape; the
//!   reasons in the second are a documented, closed set. The prose is dropped.
//!
//! Two shapes are deliberately read more loosely than the rest. `cloud/status` is accepted
//! both nested and bare, and `cloud/explore` both as `{"files": [...]}` and as a plain array
//! of addresses, because the provider's published document and its maintained clients describe
//! each of those differently and an answer in the other shape would otherwise read as an empty
//! job rather than as a job that finished.
//!
//! **Nothing here has been run against a live Offcloud account.** The shapes come from the
//! provider's documentation and from two independent clients of it; `docs/roadmap/jobs/
//! 120-02-offcloud.md` records the run against a real account as open.

use serde::Deserialize;

use crate::messages;

/// The vault reference the Offcloud provider keeps its API key under. The value never reaches
/// this plugin.
pub const TOKEN_REFERENCE: &str = "offcloud_api_key";

pub const API_BASE: &str = "https://offcloud.com/api";

/// `POST /api/cloud`: hands a magnet or an address to the account's cloud.
pub const SUBMIT_PATH: &str = "/cloud";
/// `POST /api/cloud/status`: where one request stands.
pub const STATUS_PATH: &str = "/cloud/status";
/// `GET /api/cloud/history`: what the account already holds.
pub const HISTORY_PATH: &str = "/cloud/history";
/// `GET /api/cloud/explore/{requestId}`: the file tree of a finished request.
pub const EXPLORE_PATH: &str = "/cloud/explore";
/// `POST /api/cloud/remove`: removes requests at the provider.
///
/// The one call here that sends JSON rather than the form body the published document asks
/// for, because its parameter is a *list* of request identifiers and a form has no unambiguous
/// spelling for one. Reached from a confirmed request and from nowhere else.
pub const REMOVE_PATH: &str = "/cloud/remove";

/// How many history entries `adopt` reads before giving up on finding the key.
///
/// This exists for a submit that was lost seconds ago, which is the newest entry there is, so
/// reading further would cost requests to answer a question already answered. An adoption that
/// does not find it falls through to the host's attempt ceiling rather than paging for ever.
pub const ADOPT_PAGE: usize = 100;

/// Most entries one finished job may contribute. The host bounds this again; the bound here is
/// about the invocation's own memory, before anything crosses the boundary.
pub const MAX_ENTRIES: usize = 2_000;

/// `POST /api/cloud`.
#[derive(Default, Deserialize)]
pub struct CreatedRequest {
    #[serde(default, alias = "requestId")]
    pub request_id: Option<String>,
    #[serde(default, alias = "fileName", alias = "filename")]
    pub file_name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// One entry of `GET /api/cloud/history`.
#[derive(Default, Deserialize)]
pub struct HistoryEntry {
    #[serde(default, alias = "requestId")]
    pub request_id: Option<String>,
    /// What the request was started from, as Offcloud recorded it. Compared with the content
    /// key by putting it through the very derivation a fresh source goes through.
    #[serde(default, alias = "originalLink")]
    pub original_link: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// The body of `POST /api/cloud/status`, read in both the shapes it is described in.
#[derive(Default, Deserialize)]
pub struct StatusEnvelope {
    #[serde(default)]
    pub status: Option<StatusOrWord>,
}

/// `status` is an object in the maintained clients and a bare word in the published examples.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum StatusOrWord {
    Detail(Box<RequestStatus>),
    Word(String),
}

/// What `POST /api/cloud/status` says about one request.
#[derive(Default, Deserialize)]
pub struct RequestStatus {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, alias = "fileName", alias = "filename")]
    pub file_name: Option<String>,
    /// Bytes fetched so far, as Offcloud counts them.
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default, alias = "fileSize")]
    pub file_size: Option<f64>,
    #[serde(default, alias = "serverType")]
    pub server_type: Option<String>,
    /// The address of the finished file, when the job produced exactly one.
    #[serde(default)]
    pub url: Option<String>,
}

/// The object shape of `GET /api/cloud/explore/{requestId}`.
#[derive(Default, Deserialize)]
pub struct ExploreFiles {
    #[serde(default)]
    pub files: Vec<ExploreFile>,
}

/// The files a finished job holds, read out of whichever shape the answer arrived in.
///
/// Three shapes are in the field and the provider's published document describes only one of
/// them: `{"files": [{path, size, url}]}`, a bare array of addresses (`format=simple`), and a
/// bare array of the same objects. The branch is taken on the document itself rather than left
/// to an untagged enum, because an untagged enum decides this by trying variants in order and
/// **a struct can be deserialised from a sequence** — so `["a", "b"]` could quietly become a
/// `files` list of length zero, and a finished job would arrive as an empty package with
/// nothing to explain why. That is exactly what it did before this was made explicit.
///
/// An answer in no shape at all is an empty list rather than a failure: a job with one file has
/// nothing to explore at all, and the address `cloud/status` carries is the answer for it.
#[must_use]
pub fn explore_files(body: &[u8]) -> Vec<ExploreFile> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    if value.is_array() {
        if let Ok(files) = serde_json::from_value::<Vec<ExploreFile>>(value.clone()) {
            return files;
        }
        return serde_json::from_value::<Vec<String>>(value)
            .map(|urls| {
                urls.into_iter()
                    .map(|url| ExploreFile {
                        url: Some(url),
                        ..ExploreFile::default()
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    serde_json::from_value::<ExploreFiles>(value)
        .map(|body| body.files)
        .unwrap_or_default()
}

/// One file inside a finished job.
#[derive(Clone, Default, Deserialize)]
pub struct ExploreFile {
    #[serde(default)]
    pub url: Option<String>,
    /// Where it sat inside the job, as a relative path. This is the folder structure, and the
    /// reason `poll` asks `explore` at all rather than taking the single address from `status`.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// The two refusal shapes, read out of one answer.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default, alias = "notAvailable")]
    pub not_available: Option<String>,
}

/// Where a cloud request stands, in the vocabulary of `interface remote-job` rather than of
/// Offcloud.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Doing something that needs nobody, with a suggested wait in seconds.
    Preparing(u64),
    /// Fetching, with a suggested wait.
    Working(u64),
    /// Finished; the addresses are read in a second call.
    Ready,
    /// The provider ended it, with the code and text that say how.
    Failed((&'static str, &'static str)),
}

/// Maps one of Offcloud's documented request states.
///
/// Three of the choices are worth stating rather than reading out of the table:
///
/// - **There is no `AwaitingChoice` here at all.** Offcloud fetches the whole of what it was
///   given and says afterwards what was in it, so there is never a moment at which nothing
///   moves until a person has chosen. A plugin that invented that question would be asking
///   people to answer something the provider will ignore.
/// - `queued` is `Preparing`, not `Working`. Nothing is being fetched yet, and a progress bar
///   at zero that does not move for an hour is worse than no progress bar.
/// - An unknown word is `Preparing` and not a failure. Providers add states, and a plugin that
///   failed a job on a word it did not recognise would throw away a download going perfectly
///   well.
#[must_use]
pub fn stage_of(status: &str) -> Stage {
    match status.trim().to_ascii_lowercase().as_str() {
        "downloaded" | "finished" | "completed" => Stage::Ready,
        "downloading" => Stage::Working(30),
        "created" => Stage::Preparing(10),
        "queued" | "pending" => Stage::Preparing(30),
        "error" => Stage::Failed(messages::JOB_FAILED),
        "canceled" | "cancelled" => Stage::Failed(messages::JOB_CANCELED),
        _ => Stage::Preparing(60),
    }
}

/// The status word an answer carries, whichever of the two shapes it came in.
#[must_use]
pub fn status_word(envelope: &StatusEnvelope) -> Option<&str> {
    match envelope.status.as_ref()? {
        StatusOrWord::Detail(detail) => detail.status.as_deref(),
        StatusOrWord::Word(word) => Some(word.as_str()),
    }
}

/// The detail an answer carries, when it came in the shape that has one.
#[must_use]
pub fn status_detail(envelope: &StatusEnvelope) -> Option<&RequestStatus> {
    match envelope.status.as_ref()? {
        StatusOrWord::Detail(detail) => Some(detail),
        StatusOrWord::Word(_) => None,
    }
}

/// Bytes fetched against bytes expected, in the thousandths the contract carries.
///
/// `None` rather than zero when either figure is missing or the total is zero: a progress bar
/// that reads 0 % because nothing was measured is a worse answer than no progress bar, and the
/// contract has a way to say "not measured".
#[must_use]
pub fn permille(done: Option<f64>, total: Option<f64>) -> Option<u16> {
    let (done, total) = (done?, total?);
    if !done.is_finite() || !total.is_finite() || total <= 0.0 {
        return None;
    }
    let scaled = (done / total * 1_000.0).round().clamp(0.0, 1_000.0);
    // The clamp above bounds the value into u16 range before the cast, so nothing is lost.
    Some(scaled as u16)
}

/// Whether a provider-supplied identifier is safe to put in a request path.
///
/// It comes back from Offcloud and goes out again in a URL, so it is checked rather than
/// trusted: an identifier carrying a slash or a dot segment would be a request to somewhere
/// else on the very host this plugin is allowed to reach.
#[must_use]
pub fn is_safe_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Where one finished file belongs: its bare name, and the path it sat on inside the job.
///
/// The job's own name is the root of that path, which is what turns "the magnet I pasted" into
/// the package a person expects — the same rule a crawler's `package-hint` follows, with the
/// remote job standing in for the folder. A leading segment that *is* the job name is not
/// repeated, because Offcloud already roots a multi-file job's paths at it and without this
/// every package would land in a `Name/Name` folder.
#[must_use]
pub fn place(job_name: &str, path: &str) -> (Option<String>, Option<String>) {
    let mut segments: Vec<&str> = path
        .split('/')
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

/// The file name inside an address, for a job that named none.
///
/// Last resort. A finished download with no name at all is a row a person cannot recognise, so
/// the address's last segment is used — percent-escapes and a query string removed — and
/// nothing is invented when even that is empty.
///
/// The path is taken after the authority rather than from the whole address, because an
/// address with no path at all has no last segment: scanning back from the end of
/// `https://host/` finds `https:` and would name somebody's download that.
#[must_use]
pub fn name_from_url(url: &str) -> Option<String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let after_scheme = without_query
        .split_once("://")
        .map_or(without_query, |(_, rest)| rest);
    let path = after_scheme.split_once('/')?.1;
    let last = path.rsplit('/').find(|part| !part.is_empty())?;
    let decoded = percent_decode(last);
    (!decoded.is_empty()).then_some(decoded)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(hex) = bytes.get(at + 1..at + 3)
            && let Ok(text) = std::str::from_utf8(hex)
            && let Ok(byte) = u8::from_str_radix(text, 16)
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How a refusal is classified, without depending on either failure representation.
#[derive(Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AccountInvalid,
    RateLimited(Option<u64>),
    Unsupported,
}

/// A classified refusal.
#[derive(Debug)]
pub struct ApiFailure {
    pub kind: ErrorKind,
    pub code: &'static str,
    pub message: String,
    pub params: Vec<(&'static str, String)>,
}

/// How long a provider-side outage is waited out. Five minutes, the figure the resolver
/// sibling and the other multihoster plugins settled on.
const BUSY_SECONDS: u64 = 300;

/// How long an exhausted allowance is waited out.
const QUOTA_SECONDS: u64 = 3600;

fn plain(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

fn with_param(
    kind: ErrorKind,
    (code, message): (&'static str, &str),
    name: &'static str,
    value: String,
) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: vec![(name, value)],
    }
}

/// What is left of a provider's word once everything that is not code-shaped is gone.
///
/// Offcloud answers a refusal with a sentence, and a sentence can quote whatever was sent to
/// it. So the value is kept only when the whole of it is a short, code-shaped token, and
/// dropped whole otherwise: filtering an answer that echoed a credential would keep its digits.
#[must_use]
pub fn sanitize_error(reason: &str) -> Option<String> {
    let reason = reason.trim();
    let code_shaped = !reason.is_empty()
        && reason.len() <= 40
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    code_shaped.then(|| reason.to_ascii_lowercase())
}

/// Classifies the one stable word Offcloud puts in `error`, and buckets everything else.
#[must_use]
pub fn classify_error(reason: &str) -> ApiFailure {
    match sanitize_error(reason).as_deref() {
        Some("noauth") => plain(ErrorKind::AccountInvalid, messages::AUTH_INVALID),
        Some(token) => with_param(
            ErrorKind::Permanent,
            messages::API_ERROR,
            "api_code",
            token.to_owned(),
        ),
        None => plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::API_ERROR,
        ),
    }
}

/// Classifies the closed set of `not_available` reasons.
///
/// None of them is a fault of the source: each says the account's plan does not cover this kind
/// of download.
#[must_use]
pub fn classify_not_available(reason: &str) -> ApiFailure {
    let token = sanitize_error(reason).unwrap_or_else(|| "unknown".to_owned());
    with_param(
        ErrorKind::Unsupported,
        messages::ADDON_REQUIRED,
        "addon",
        token,
    )
}

/// The refusal an answer carries, or an empty envelope when it carries none.
///
/// **Only a JSON object can be a refusal**, and that has to be checked rather than assumed:
/// serde deserialises a struct from a *sequence* as readily as from a map, taking the elements
/// in field order. So `["https://a", "https://b"]` read straight into [`ErrorEnvelope`] becomes
/// `{error: "https://a", not_available: "https://b"}` -- a refusal invented out of a perfectly
/// good answer. Both `cloud/explore` and `cloud/history` answer with arrays, and before this
/// guard a finished job whose file tree came back in the bare-address shape was classified as
/// a missing add-on and arrived as an empty package.
#[must_use]
pub fn error_envelope(body: &[u8]) -> ErrorEnvelope {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .filter(serde_json::Value::is_object)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// The failure an answer describes, or `None` when it describes none.
///
/// Three sources disagree often enough that the order between them has to be written down
/// rather than fallen into:
///
/// 1. `not_available` wins outright. It is the one answer that names something a person can
///    do, and an answer carrying it and an error is still about the add-on.
/// 2. **A code-shaped `error` beats the status.** Offcloud answers a refusal with a 200 as
///    readily as with a 401, so a status-first rule would read `NOAUTH` on a 200 as no refusal
///    at all.
/// 3. **The status beats prose.** A 429 is a spent request budget whatever sentence rides
///    along with it, and reading that sentence instead would turn the one answer that carries
///    a `Retry-After` into a guess. This is the order the fixtures caught: before it,
///    `{"error": "Too many requests, please slow down."}` on a 429 was a five-minute wait
///    rather than the two minutes the header asked for.
/// 4. Prose on a 2xx is left: something refused this and nothing says what.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if let Some(reason) = envelope.not_available.as_deref() {
        return Some(classify_not_available(reason));
    }
    if let Some(reason) = envelope
        .error
        .as_deref()
        .filter(|reason| sanitize_error(reason).is_some())
    {
        return Some(classify_error(reason));
    }
    if let Err(failure) = ensure_http_status(status, retry_after) {
        return Some(failure);
    }
    envelope.error.as_deref().map(classify_error)
}

/// Maps an HTTP status no document explains.
///
/// # Errors
///
/// The classified refusal, for every status that is not a 2xx.
pub fn ensure_http_status(status: u16, retry_after: Option<u64>) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(plain(ErrorKind::AccountInvalid, messages::AUTH_INVALID)),
        404 | 410 => Err(plain(ErrorKind::Offline, messages::JOB_GONE)),
        429 => Err(ApiFailure {
            kind: ErrorKind::RateLimited(Some(retry_after.unwrap_or(QUOTA_SECONDS))),
            code: messages::RATE_LIMITED.0,
            message: messages::RATE_LIMITED.1.to_owned(),
            params: Vec::new(),
        }),
        500..=599 => Err(plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_ERROR,
        )),
        other => Err(with_param(
            ErrorKind::Permanent,
            messages::HTTP_ERROR,
            "status",
            other.to_string(),
        )),
    }
}

/// Reads a `Retry-After` header stated in seconds. A date-shaped one is ignored rather than
/// guessed at: a wrong wait is worse than the bucket's own default.
#[must_use]
pub fn retry_after_seconds(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.trim().parse::<u64>().ok())
}

/// `application/x-www-form-urlencoded` body, as the published API asks for its parameters.
///
/// Percent-encodes by hand rather than pulling a URL crate in for two fields: one of them is a
/// magnet, which is full of `&`, `=` and `:`, and a body that did not encode them would submit
/// a truncated address.
#[must_use]
pub fn form_body(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut body = String::new();
    for (name, value) in pairs {
        if !body.is_empty() {
            body.push('&');
        }
        encode_into(&mut body, name);
        body.push('=');
        encode_into(&mut body, value);
    }
    body.into_bytes()
}

fn encode_into(body: &mut String, value: &str) {
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                body.push(char::from(*byte));
            }
            _ => {
                use std::fmt::Write;
                // Writing into a String cannot fail; the result is discarded rather than
                // unwrapped, because `unwrap_used` is denied outside tests.
                let _ = write!(body, "%{byte:02X}");
            }
        }
    }
}

/// The JSON body `cloud/remove` takes: one identifier in the list it expects.
///
/// Built by hand rather than with a serialiser, because the only value in it has already been
/// checked by [`is_safe_request_id`] and therefore carries nothing that would need escaping.
#[must_use]
pub fn remove_body(request_id: &str) -> Vec<u8> {
    format!("{{\"requestIds\":[\"{request_id}\"]}}").into_bytes()
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

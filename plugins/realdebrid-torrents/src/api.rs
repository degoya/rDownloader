//! Target-independent Real-Debrid `torrents/*` logic: response shapes, the state machine over
//! the provider's own words, and failure classification.
//!
//! Written against the published API document at <https://api.real-debrid.com/>:
//!
//! - **Auth flavour**: `Authorization: Bearer <token>`, the same token the resolver sibling
//!   uses. This plugin never sees it: every request carries the template
//!   `{{secret:realdebrid_access_token}}` and the host expands it towards
//!   `api.real-debrid.com` and nowhere else.
//! - **Submit**: `POST /torrents/addMagnet` with a form body carrying `magnet`, and
//!   `PUT /torrents/addTorrent` with the container's bytes as the body. Both answer
//!   `{"id": "...", "uri": "..."}`. **Neither is idempotent** — each call creates another
//!   torrent in the account — which is the single fact the whole design in
//!   `docs/adr/0003-a-job-that-runs-at-the-provider.md` is built around.
//! - **Adopt**: `GET /torrents` lists what the account already holds, each entry carrying the
//!   info `hash`. That is what closes the window between a submit going out and its answer
//!   coming back.
//! - **Poll**: `GET /torrents/info/{id}` carries `status`, `progress`, `files` and `links`.
//! - **Choose**: `POST /torrents/selectFiles/{id}` with `files=<comma separated ids>`. It has
//!   to happen *before* `links` carries anything at all, which is why a person stands between
//!   two calls here and why this is not a crawl.
//! - **Discard**: `DELETE /torrents/delete/{id}`, and only ever from a confirmed request.
//! - **Errors**: `{"error": "<sentence>", "error_code": <number>}`. The number is stable and
//!   documented, the sentence is not; the number travels and the sentence is dropped, exactly
//!   as the resolver sibling does it.
//! - **Rate limit**: 250 requests a minute for the whole account, refused ones included. The
//!   polling shares that budget with the resolver unrestricting this very job's links, which
//!   is why the suggested waits below are generous rather than eager.

use serde::Deserialize;

use crate::messages;

/// The vault reference the Real-Debrid provider keeps its access token under. The value never
/// reaches this plugin.
pub const TOKEN_REFERENCE: &str = "realdebrid_access_token";

pub const API_BASE: &str = "https://api.real-debrid.com/rest/1.0";

/// How many of the account's torrents `adopt` reads before giving up on finding the hash.
///
/// `GET /torrents` pages, and a person with a thousand torrents would otherwise cost a
/// thousand requests against a budget of 250 a minute. One page is enough for the case this
/// exists for — a submit that was lost seconds ago is the newest entry there is — and an
/// adoption that does not find it falls through to the host's attempt ceiling rather than
/// paging for ever.
pub const ADOPT_PAGE: u32 = 100;

/// `POST /torrents/addMagnet` and `PUT /torrents/addTorrent`.
#[derive(Default, Deserialize)]
pub struct AddedTorrent {
    #[serde(default)]
    pub id: Option<String>,
    // `uri` is deliberately not read: it is the address of the torrent resource, not of any
    // file, and queuing it would download a JSON document.
}

/// One entry of `GET /torrents`.
#[derive(Default, Deserialize)]
pub struct ListedTorrent {
    #[serde(default)]
    pub id: Option<String>,
    /// The info hash, as Real-Debrid spells it. Compared case-insensitively with the content
    /// key, because the two come from different places and neither promises a case.
    #[serde(default)]
    pub hash: Option<String>,
}

/// One file inside a torrent, from `GET /torrents/info/{id}`.
#[derive(Default, Deserialize)]
pub struct TorrentFile {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub bytes: Option<u64>,
    /// `1` when Real-Debrid already considers the file selected.
    #[serde(default)]
    pub selected: Option<i64>,
}

/// `GET /torrents/info/{id}`.
#[derive(Default, Deserialize)]
pub struct TorrentInfo {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Percent, 0 to 100.
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub speed: Option<u64>,
    #[serde(default)]
    pub files: Vec<TorrentFile>,
    /// The generated links, in the order of the *selected* files. Empty until a selection has
    /// been made, which is the contract's whole reason for asking a person first.
    #[serde(default)]
    pub links: Vec<String>,
}

/// The failure envelope every endpoint answers a refusal with.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    /// The provider's own sentence. Read so its presence can be detected and never forwarded.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_code: Option<i64>,
}

/// Where a torrent stands, in the vocabulary of `interface remote-job` rather than of
/// Real-Debrid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Doing something that needs nobody, with a suggested wait in seconds.
    Preparing(u64),
    /// `waiting_files_selection`: nothing moves until a person has chosen.
    AwaitingChoice,
    /// `downloading`: fetching, with a suggested wait.
    Working(u64),
    /// `downloaded`: finished.
    Ready,
    /// The provider ended it, with the code and text that say how.
    Failed((&'static str, &'static str)),
}

/// Maps one of Real-Debrid's ten documented torrent states.
///
/// The mapping is where the provider's vocabulary stops and the contract's begins, and three
/// of the choices are worth stating rather than reading out of the table:
///
/// - `queued` is `Preparing`, not `Working`. Nothing is being fetched yet and a progress bar
///   at zero percent that does not move for an hour is worse than no progress bar.
/// - `compressing` and `uploading` are `Preparing` too. They happen *after* the download at
///   Real-Debrid's end and before the links exist, so from here they are indistinguishable
///   from the conversion at the start: something is happening and nobody is needed.
/// - An unknown word is `Preparing` and not a failure. Real-Debrid has added states before,
///   and a plugin that failed a job on a word it did not recognise would throw away a torrent
///   that was going perfectly well.
#[must_use]
pub fn stage_of(status: &str) -> Stage {
    match status {
        "waiting_files_selection" => Stage::AwaitingChoice,
        "downloading" => Stage::Working(30),
        "downloaded" => Stage::Ready,
        "magnet_conversion" => Stage::Preparing(10),
        "queued" | "compressing" | "uploading" => Stage::Preparing(30),
        "magnet_error" => Stage::Failed(messages::MAGNET_REJECTED),
        "virus" => Stage::Failed(messages::CONTENT_REFUSED),
        "dead" => Stage::Failed(messages::TORRENT_DEAD),
        "error" => Stage::Failed(messages::TORRENT_FAILED),
        _ => Stage::Preparing(60),
    }
}

/// Percent as Real-Debrid states it, in the thousandths the contract carries.
#[must_use]
pub fn permille(progress: Option<f64>) -> Option<u16> {
    let progress = progress?;
    if !progress.is_finite() {
        return None;
    }
    let scaled = (progress * 10.0).round().clamp(0.0, 1_000.0);
    // The clamp above bounds the value into u16 range before the cast, so nothing is lost.
    Some(scaled as u16)
}

/// Whether Real-Debrid already considers a file selected.
#[must_use]
pub fn is_selected(selected: Option<i64>) -> bool {
    selected.is_some_and(|value| value != 0)
}

/// Pairs the links a finished torrent produced with the files they belong to.
///
/// Real-Debrid answers `links` in the order of the **selected** files and says nothing else
/// about which is which, so the pairing is positional and there is no other way to do it. What
/// can be done is to refuse to guess when the two do not line up: a link with no file keeps
/// its address and loses its name, rather than borrowing the name of somebody else's file.
#[must_use]
pub fn pair_links<'a>(
    links: &'a [String],
    files: &'a [TorrentFile],
) -> Vec<(&'a str, Option<&'a TorrentFile>)> {
    let selected: Vec<&TorrentFile> = files
        .iter()
        .filter(|file| is_selected(file.selected))
        .collect();
    let aligned = selected.len() == links.len();
    links
        .iter()
        .enumerate()
        .map(|(index, link)| {
            let file = if aligned {
                selected.get(index).copied()
            } else {
                None
            };
            (link.as_str(), file)
        })
        .collect()
}

/// Where one finished file belongs: its bare name, and the path it sat on inside the job.
///
/// The torrent's own name is the root of that path, which is what turns "the magnet I pasted"
/// into the package a person expects — the same rule a crawler's `package-hint` follows, with
/// the remote job standing in for the folder. Real-Debrid already roots every path of a
/// multi-file torrent at that name (`/Show.S01/ep01.mkv` under a torrent called `Show.S01`),
/// so a leading segment that *is* the name is not repeated: without this every package a
/// remote job produced landed in a `Show.S01/Show.S01` folder (RD-108-03).
#[must_use]
pub fn place(torrent_name: &str, path: &str) -> (Option<String>, Option<String>) {
    let mut segments: Vec<&str> = path
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect();
    let file_name = segments.pop().map(str::to_owned);
    let mut place = Vec::new();
    let torrent_name = torrent_name.trim();
    if !torrent_name.is_empty() {
        place.push(torrent_name);
        if segments.first().is_some_and(|first| *first == torrent_name) {
            segments.remove(0);
        }
    }
    place.extend(segments);
    let hint = (!place.is_empty()).then(|| place.join("/"));
    (file_name, hint)
}

/// Whether a provider-supplied identifier is safe to put in a request path.
///
/// It comes back from Real-Debrid and goes out again in a URL, so it is checked rather than
/// trusted: an identifier carrying a slash or a dot segment would be a request to somewhere
/// else on the very host this plugin is allowed to reach.
#[must_use]
pub fn is_safe_remote_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The comma-separated `files` value `torrents/selectFiles/{id}` takes.
#[must_use]
pub fn selection_body(chosen: &[u32]) -> Vec<u8> {
    let mut ids: Vec<u32> = chosen.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let joined = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    format!("files={joined}").into_bytes()
}

/// `application/x-www-form-urlencoded` body for `POST /torrents/addMagnet`.
///
/// Percent-encodes by hand rather than pulling a URL crate in for one field: a magnet is full
/// of `&`, `=` and `:`, and a body that did not encode them would submit a truncated address.
#[must_use]
pub fn magnet_body(magnet: &str) -> Vec<u8> {
    let mut body = String::from("magnet=");
    for byte in magnet.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                body.push(char::from(*byte));
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(body, "%{byte:02X}");
            }
        }
    }
    body.into_bytes()
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
    IpBlocked,
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

/// How long an exhausted quota is waited out.
const QUOTA_SECONDS: u64 = 3600;

fn coded(kind: ErrorKind, (code, message): (&'static str, &str), api_code: i64) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: vec![("api_code", api_code.to_string())],
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

/// Classifies a documented `error_code`.
///
/// The same buckets the resolver sibling uses, plus the two the torrent endpoints add: 25
/// ("infringing file") and 33 ("torrent already active") are not general failures, and the
/// second is not a failure at all — it is the provider saying the job already exists, which
/// the adoption path turns into a handle rather than into an error.
#[must_use]
pub fn classify_error(api_code: i64, retry_after: Option<u64>) -> ApiFailure {
    match api_code {
        // Real-Debrid's eight token and permission codes are contiguous, so say so.
        8..=15 => coded(ErrorKind::AccountInvalid, messages::AUTH_INVALID, api_code),
        7 | 24 | 35 => coded(ErrorKind::Offline, messages::TORRENT_GONE, api_code),
        16 | 20 => coded(ErrorKind::Unsupported, messages::NOT_PERMITTED, api_code),
        25 | 26 => coded(ErrorKind::Permanent, messages::CONTENT_REFUSED, api_code),
        6 | 17 | 19 | 21 => coded(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_BUSY,
            api_code,
        ),
        18 | 23 | 36 => coded(
            ErrorKind::RateLimited(Some(QUOTA_SECONDS)),
            messages::LIMIT_REACHED,
            api_code,
        ),
        22 => coded(ErrorKind::IpBlocked, messages::IP_NOT_ALLOWED, api_code),
        5 | 34 => coded(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
            api_code,
        ),
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR.0,
            message: messages::api_error(other),
            params: vec![("api_code", other.to_string())],
        },
    }
}

/// The failure an answer describes, or `None` when it describes none.
///
/// An answer is a failure when it carries an `error_code`, whatever its HTTP status; a 2xx
/// carrying one is still a refusal, and a 4xx carrying none is classified by its status alone.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if let Some(api_code) = envelope.error_code {
        return Some(classify_error(api_code, retry_after));
    }
    if envelope.error.is_some() || !(200..=299).contains(&status) {
        return ensure_http_status(status, retry_after).err();
    }
    None
}

/// Maps an HTTP status no `error_code` explains.
///
/// # Errors
///
/// The classified refusal, for every status that is not a 2xx.
pub fn ensure_http_status(status: u16, retry_after: Option<u64>) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(plain(ErrorKind::AccountInvalid, messages::AUTH_INVALID)),
        404 | 410 => Err(plain(ErrorKind::Offline, messages::TORRENT_GONE)),
        429 => Err(plain(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
        )),
        451 => Err(plain(ErrorKind::Permanent, messages::CONTENT_REFUSED)),
        500..=599 => Err(plain(ErrorKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR.0,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
    }
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

//! Target-independent Put.io `transfers/*` and `files/*` logic: response shapes, the state
//! machine over the provider's own words, and failure classification.
//!
//! Written against Put.io's published API v2:
//!
//! - **Auth flavour**: `Authorization: Bearer <token>`, the same token the resolver sibling
//!   uses. This plugin never sees it: every request carries the template
//!   `{{secret:putio_access_token}}` and the host expands it towards `api.put.io` and nowhere
//!   else.
//! - **Submit**: `POST /v2/transfers/add` with a form body carrying `url`, which takes a magnet
//!   address. It answers `{"transfer": {...}}`. **It is not idempotent** — each call creates
//!   another transfer in the account — which is the single fact the whole design in
//!   `docs/adr/0003-a-job-that-runs-at-the-provider.md` is built around.
//! - **Adopt**: `GET /v2/transfers/list` lists what the account already holds. Put.io states a
//!   transfer's torrent in up to three fields (`hash`, `magneturi`, `source`) and does not fill
//!   all of them for every transfer, so all three are read and asked the same question: is this
//!   our twenty bytes. That is what closes the window between a submit going out and its answer
//!   coming back.
//! - **Poll**: `GET /v2/transfers/{id}` carries `status`, `percent_done`, `down_speed`,
//!   `estimated_time` and, once it is finished, `file_id`.
//! - **Hand over**: `GET /v2/files/{id}` and `GET /v2/files/list?parent_id={id}` walk what the
//!   transfer produced, so every file's name and size is known before any of them is offered.
//! - **Discard**: `POST /v2/transfers/cancel` with `transfer_ids`, and only ever from a
//!   confirmed request.
//! - **Errors**: `{"error_type": "<WORD>", "error_message": "<sentence>"}`. The word is stable
//!   and documented, the sentence is not; the word travels and the sentence is dropped.
//!
//! **Put.io offers no file selection.** A torrent is fetched whole and its files exist only
//! once it has finished, so there is no moment at which the provider could be told to leave one
//! out. `poll` therefore never answers `awaiting-choice`: it answers `ready` with the complete
//! tree, every file carrying its name, its size and the folder it sat in, and the choice is
//! made in the LinkGrabber before anything is downloaded locally. What that gives up against
//! the Real-Debrid model is the ability to stop Put.io itself from fetching a file; what it
//! keeps is that nobody has to guess, and that nothing at the provider is deleted to express a
//! choice.

use serde::Deserialize;

use crate::messages;
use putio_common::reason::ErrorEnvelope;

/// The vault reference the Put.io provider keeps its access token under. The value never
/// reaches this plugin.
pub const TOKEN_REFERENCE: &str = "putio_access_token";

/// How many of the account's transfers `adopt` reads before giving up on finding the hash.
///
/// `GET /v2/transfers/list` answers with the account's active transfers, newest first. This
/// exists for a submit that was lost seconds ago, which is the newest entry there is, so an
/// adoption that does not find it in this many falls through to the host's attempt ceiling
/// rather than walking an entire account.
pub const ADOPT_LIMIT: usize = 200;

/// Most entries one folder listing asks for. Put.io's own maximum.
pub const LIST_PAGE: u32 = 1000;

/// Most requests the walk over a finished transfer may make. One per folder plus one for the
/// root, so this is a bound on how deep and how wide a torrent's folder tree may be before
/// rDownloader refuses it rather than spending an account's request budget on one job.
pub const MAX_TREE_REQUESTS: usize = 100;

/// Most files one finished transfer hands over. A remote job becomes one LinkGrabber batch,
/// and a batch of ten thousand links is not a package anybody reads.
pub const MAX_FILES: usize = 2000;

/// Deepest folder nesting the walk follows.
pub const MAX_DEPTH: u32 = 16;

/// One transfer, as Put.io states it.
#[derive(Debug, Default, Deserialize)]
pub struct Transfer {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Percent, 0 to 100.
    #[serde(default)]
    pub percent_done: Option<i64>,
    #[serde(default)]
    pub down_speed: Option<u64>,
    #[serde(default)]
    pub estimated_time: Option<u64>,
    /// The folder or file the finished transfer produced. Absent until it has one.
    #[serde(default)]
    pub file_id: Option<u64>,
    /// The info hash, where Put.io states it as its own field.
    #[serde(default)]
    pub hash: Option<String>,
    /// The magnet Put.io holds for this transfer.
    #[serde(default)]
    pub magneturi: Option<String>,
    /// What the transfer was created from.
    #[serde(default)]
    pub source: Option<String>,
    // `error_message` is deliberately not read: it is Put.io's own sentence about a failed
    // transfer, and `TRANSFER_FAILED` says the same thing in a language somebody reads.
}

impl Transfer {
    /// Whether this transfer is the one `content_key` names.
    ///
    /// All three fields are asked, because Put.io fills different ones for a magnet, for an
    /// uploaded torrent and for a transfer it created itself — and a comparison that read only
    /// the field this account happens not to have would adopt nothing and submit twice.
    #[must_use]
    pub fn carries(&self, content_key: &str) -> bool {
        [
            self.hash.as_deref(),
            self.magneturi.as_deref(),
            self.source.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(crate::source::info_hash_within)
        .any(|hash| hash == content_key)
    }
}

/// `POST /v2/transfers/add` and `GET /v2/transfers/{id}`.
#[derive(Debug, Default, Deserialize)]
pub struct TransferResponse {
    #[serde(default)]
    pub transfer: Option<Transfer>,
}

/// `GET /v2/transfers/list`.
#[derive(Debug, Default, Deserialize)]
pub struct TransferListResponse {
    #[serde(default)]
    pub transfers: Vec<Transfer>,
}

/// One file or folder in the account.
#[derive(Debug, Default, Deserialize)]
pub struct FileRecord {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub file_type: Option<String>,
}

impl FileRecord {
    /// Whether this record is a folder rather than something with bytes in it.
    ///
    /// Put.io marks a folder in two fields and does not always fill both, so both are read: a
    /// folder let through as a file would become a download of a JSON document.
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.file_type.as_deref() == Some("FOLDER")
            || self.content_type.as_deref() == Some("application/x-directory")
    }
}

/// `GET /v2/files/{id}`.
#[derive(Debug, Default, Deserialize)]
pub struct FileResponse {
    #[serde(default)]
    pub file: Option<FileRecord>,
}

/// `GET /v2/files/list?parent_id={id}`.
#[derive(Debug, Default, Deserialize)]
pub struct FileListResponse {
    #[serde(default)]
    pub files: Vec<FileRecord>,
}

/// Where a transfer stands, in the vocabulary of `interface remote-job` rather than of Put.io.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Doing something that needs nobody, with a suggested wait in seconds.
    Preparing(u64),
    /// `DOWNLOADING`: fetching.
    Working,
    /// Finished, and the files exist.
    Ready,
    /// Put.io ended it, with the code and text that say how.
    Failed((&'static str, &'static str)),
}

/// Maps one of Put.io's transfer states.
///
/// Three of the choices are worth stating rather than reading out of the table:
///
/// - `SEEDING` is `Ready`, not `Working`. The files exist and are complete; Put.io is giving
///   back to the swarm, which is its business and not a reason to make somebody wait.
/// - `COMPLETING` is `Preparing`. It happens after the download at Put.io's end and before the
///   files are there, so from here it is indistinguishable from the preparation at the start:
///   something is happening and nobody is needed.
/// - An unknown word is `Preparing` and not a failure. Put.io has added states before, and a
///   plugin that failed a job on a word it did not recognise would throw away a transfer that
///   was going perfectly well.
#[must_use]
pub fn stage_of(status: &str) -> Stage {
    match status {
        "DOWNLOADING" => Stage::Working,
        "COMPLETED" | "SEEDING" => Stage::Ready,
        "IN_QUEUE" | "WAITING" | "WAITING_FOR_PEERS" => Stage::Preparing(30),
        "PREPARING" | "PREPARING_DOWNLOAD" => Stage::Preparing(10),
        "COMPLETING" => Stage::Preparing(15),
        "ERROR" => Stage::Failed(messages::TRANSFER_FAILED),
        "CANCELLING" | "CANCELLED" | "CANCELED" => Stage::Failed(messages::TRANSFER_CANCELLED),
        _ => Stage::Preparing(60),
    }
}

/// Percent as Put.io states it, in the thousandths the contract carries.
#[must_use]
pub fn permille(percent_done: Option<i64>) -> Option<u16> {
    let percent = percent_done?.clamp(0, 100);
    // The clamp above bounds the value well inside u16 range before the cast.
    u16::try_from(percent * 10).ok()
}

/// Whether a provider-supplied identifier is safe to put in a request path.
///
/// It comes back from Put.io and goes out again in a URL, so it is checked rather than
/// trusted: an identifier carrying a slash or a dot segment would be a request to somewhere
/// else on the very host this plugin is allowed to reach. Put.io's transfer ids are integers,
/// so digits are the whole alphabet.
#[must_use]
pub fn is_safe_remote_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// Where one finished file belongs: the folders it sat in inside the transfer, joined.
///
/// The transfer's own root is the first segment, which is what turns "the magnet I pasted" into
/// the package a person expects — the same rule a crawler's `package-hint` follows, with the
/// remote job standing in for the folder. `None` when there is nothing to say, which is a
/// transfer of one loose file.
#[must_use]
pub fn package_hint(folders: &[String]) -> Option<String> {
    let cleaned: Vec<&str> = folders
        .iter()
        .map(|segment| segment.trim())
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect();
    (!cleaned.is_empty()).then(|| cleaned.join("/"))
}

/// The form body `POST /v2/transfers/cancel` takes.
#[must_use]
pub fn cancel_body(remote_id: &str) -> Vec<u8> {
    format!("transfer_ids={remote_id}").into_bytes()
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

/// How long a provider-side outage is waited out.
const BUSY_SECONDS: u64 = 300;

/// The wait a 429 gets when Put.io named no usable window.
const RATE_LIMIT_SECONDS: u64 = 60;

/// The longest wait a `X-RateLimit-Reset` is believed for. The header is an absolute Unix
/// timestamp, so a clock that disagrees with Put.io's could otherwise park a job for days.
const MAX_RATE_LIMIT_SECONDS: u64 = 3600;

fn failure(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// The refusal an answer describes, or `None` when it describes none.
///
/// An answer is a refusal when its status says so or when it carries an error document,
/// whichever comes first; a 2xx carrying one is still a refusal.
#[must_use]
pub fn failure_from(
    status: u16,
    reset_in_seconds: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if (200..=299).contains(&status) && !envelope.is_refusal() {
        return None;
    }
    let word = envelope.kind();
    let mut refusal = classify(status, reset_in_seconds, word.as_deref());
    if let Some(word) = word {
        refusal.params.push(("reason", word));
    }
    Some(refusal)
}

/// Maps one refusal onto the category the sweep acts on.
///
/// The status decides, because Put.io's statuses are the part that is documented and stable.
/// The word is read for two things the status alone cannot say: a 403 that is really an
/// expired token, and a refusal that is really a full account — the difference between "sign
/// in again", "this will never work" and "make room and try again".
fn classify(status: u16, reset_in_seconds: Option<u64>, word: Option<&str>) -> ApiFailure {
    if matches!(word, Some("DISK_QUOTA_EXCEEDED" | "ACCOUNT_DISK_FULL")) {
        return failure(ErrorKind::Permanent, messages::DISK_FULL);
    }
    let token_refused = matches!(
        word,
        Some("INVALID_TOKEN" | "INVALID_GRANT" | "UNAUTHORIZED")
    );
    match status {
        401 => failure(ErrorKind::AccountInvalid, messages::AUTH_INVALID),
        403 if token_refused => failure(ErrorKind::AccountInvalid, messages::AUTH_INVALID),
        403 => failure(ErrorKind::Unsupported, messages::NOT_PERMITTED),
        404 | 410 => failure(ErrorKind::Offline, messages::TRANSFER_GONE),
        429 => failure(
            ErrorKind::RateLimited(Some(reset_in_seconds.unwrap_or(RATE_LIMIT_SECONDS))),
            messages::RATE_LIMITED,
        ),
        500..=599 => failure(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_ERROR,
        ),
        status if (200..=299).contains(&status) => {
            failure(ErrorKind::Permanent, messages::API_ERROR)
        }
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR.0,
            message: messages::http_error(other),
            params: Vec::new(),
        },
    }
}

/// How long a rate-limit window still has to run, from Put.io's `X-RateLimit-Reset` and the
/// host's own clock.
///
/// The header is an absolute Unix timestamp rather than a duration, which is why this needs
/// the clock at all. Three answers are deliberately `None` rather than a guess: a header that
/// is not a number, one already in the past, and one so far away that the two clocks plainly
/// disagree. The caller then waits its own default, which is a wait somebody can reason about.
#[must_use]
pub fn rate_limit_wait(reset_header: Option<&str>, now_unix_seconds: u64) -> Option<u64> {
    let reset: u64 = reset_header?.trim().parse().ok()?;
    let remaining = reset.checked_sub(now_unix_seconds)?;
    (remaining > 0 && remaining <= MAX_RATE_LIMIT_SECONDS).then_some(remaining)
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

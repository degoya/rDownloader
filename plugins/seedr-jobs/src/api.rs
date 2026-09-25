//! Target-independent Seedr transfer logic: the response shapes, the state machine over what a
//! folder listing shows, and the failure classification.
//!
//! Written against Seedr's published REST v1, <https://www.seedr.cc/docs/api/rest/v1/>, read
//! again on 2026-09-23 as the job file asked:
//!
//! - **Auth flavour**: HTTP Basic, and nothing else — the page says so itself. This plugin
//!   never sees either half: every request carries `seedr_common::address::
//!   AUTHORIZATION_TEMPLATE` and the host builds the blob towards `www.seedr.cc` and nowhere
//!   else. The token variant the feasibility left open does not exist; the v2 and OAuth API the
//!   page has announced since its 2021 copyright still have no reference anywhere.
//! - **Submit**: `POST /rest/transfer/magnet` with a form body carrying `magnet`. **It is not
//!   idempotent** — each call creates another transfer in the account — which is the single
//!   fact the design in `docs/adr/0003-a-job-that-runs-at-the-provider.md` is built around.
//! - **Poll**: `GET /rest/folder`, the account's root listing, and *not* the documented
//!   `GET /rest/transfer/{id}`. That is the one design decision in this plugin worth arguing,
//!   and [`stage_of`] is where it is argued.
//! - **Ready**: `GET /rest/folder/{id}`, walked to the leaves. Seedr's own example demonstrates
//!   the shape (`$root_folder->folders[0]->id`, `$sub->files[0]->id`).
//! - **Discard**: `DELETE /rest/transfer/{id}`, and only ever from a confirmed request.
//!
//! **Nothing here has been run against a live Seedr account.** The shapes come from the
//! provider's documentation and its own worked example, and the method matrix behind them was
//! measured on 2026-09-22; `docs/roadmap/jobs/120-04-seedr-feasibility.md` records the run
//! against a real, premium account as open.

use seedr_common::folder::{Listing, Torrent};

use crate::messages;

/// Most entries one finished transfer may contribute. The host bounds this again; the bound
/// here is about the invocation's own memory, before anything crosses the boundary.
pub const MAX_ENTRIES: usize = 2_000;

/// Deepest a finished transfer's folder tree is walked.
///
/// A release folder is one or two levels; anything past this is either a pathological torrent
/// or a listing that answers with itself, and walking it would spend the invocation's whole
/// budget finding that out.
pub const MAX_DEPTH: u32 = 8;

/// Most folder listings one `poll` will ask for.
///
/// The real bound on the walk, and the one that matters: depth alone does not stop a folder
/// holding a thousand siblings, and each of those is a request against an account's shared
/// rate limit.
pub const MAX_LISTINGS: usize = 64;

/// How long a fresh transfer is left alone before the first poll asks again.
pub const PREPARING_SECONDS: u64 = 15;

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// API-shaped providers in this tree settled on.
const BUSY_SECONDS: u64 = 300;

/// The wait a 429 gets when Seedr states no usable `Retry-After`.
const RATE_LIMIT_SECONDS: u64 = 60;

/// The longest `Retry-After` that is believed, so a header from a proxy cannot park a job for
/// days.
const MAX_RETRY_AFTER_SECONDS: u64 = 3600;

/// Longest transfer name kept on the handle. It is Seedr's own text and it comes back as a
/// folder name to match on, so it is bounded rather than trusted.
pub const MAX_NAME_BYTES: usize = 255;

/// Where one transfer stands, in the vocabulary of `interface remote-job` rather than Seedr's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Seedr has the transfer and is not fetching yet, with a suggested wait in seconds.
    Preparing(u64),
    /// Fetching, with the progress Seedr stated, in thousandths.
    Working(Option<u16>),
    /// Finished; the files are in this folder, which is read in further calls.
    Ready(u64),
    /// Gone, with the code and text that say how.
    Failed((&'static str, &'static str)),
}

/// Where a transfer stands, read out of the account's root listing.
///
/// **The listing is the poll, and the endpoint named after polling is not.** Seedr documents
/// `GET /rest/transfer/{id}`, and it reports a *running* transfer perfectly well — but when the
/// torrent finishes, Seedr moves it out of the transfer list and turns it into a folder. From
/// the transfer endpoint that is indistinguishable from a transfer somebody deleted: both are
/// "this id is not here any more". A plugin that polled it would have to guess which of the two
/// had happened, and guessing wrong either way costs somebody a finished download or hangs a
/// dead one for ever. The root listing answers both questions in one request, because the
/// transfer and the folder it became are two entries in the same document.
///
/// Three of the choices are worth stating rather than reading out of the code:
///
/// - **100 % is not finished.** Seedr's own documentation warns that a file at 100 % is fully
///   downloaded and that *101 %* means it has been moved to a folder. So progress says nothing
///   about the end; the transfer leaving the list is what the end looks like.
/// - **A transfer with no progress yet is `Preparing`, not `Working` at zero.** A progress bar
///   at zero that does not move for an hour is worse than no progress bar.
/// - **A transfer that is gone with no folder to show for it is a failure, not a wait.** It is
///   the shape of a transfer somebody removed in Seedr's own interface, and waiting for it
///   would keep a row polling an account for ever.
#[must_use]
pub fn stage_of(listing: &Listing, transfer_id: u64, job_name: &str) -> Stage {
    if let Some(torrent) = find_transfer(listing, transfer_id) {
        return match seedr_common::folder::permille(torrent.progress) {
            Some(0) | None => Stage::Preparing(PREPARING_SECONDS),
            permille => Stage::Working(permille),
        };
    }
    match find_folder(listing, job_name) {
        Some(folder_id) => Stage::Ready(folder_id),
        None => Stage::Failed(messages::TRANSFER_GONE),
    }
}

/// The running transfer with this identifier, if the listing still holds it.
#[must_use]
pub fn find_transfer(listing: &Listing, transfer_id: u64) -> Option<&Torrent> {
    listing
        .torrents
        .iter()
        .find(|torrent| torrent.id == Some(transfer_id))
}

/// The folder a finished transfer became, matched on the name it was created under.
///
/// Matching on a name is not free of risk and the risk is stated rather than hidden: two
/// transfers of the same release in one account would produce two folders with one name, and
/// this would find the first. The alternative is worse — Seedr names no folder on the transfer
/// record until it has made one, and `job-state` is written only at submit and adopt, so a
/// plugin cannot remember an identifier it learns while polling. The host's own duplicate
/// guard is what keeps the collision rare: one content key is one transfer per account.
#[must_use]
pub fn find_folder(listing: &Listing, job_name: &str) -> Option<u64> {
    let job_name = job_name.trim();
    if job_name.is_empty() {
        return None;
    }
    listing
        .folders
        .iter()
        .find(|folder| folder.name.as_deref().map(str::trim) == Some(job_name))
        .and_then(|folder| folder.id)
}

/// The transfer identifier Seedr named when it created one.
///
/// Read out of whichever of its spellings arrived, and checked: it goes back into a request
/// path on the one host this plugin may reach.
#[must_use]
pub fn created_transfer_id(body: &[u8]) -> Option<u64> {
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let object = value.as_object()?;
    ["user_torrent_id", "torrent_id", "id"]
        .iter()
        .find_map(|name| object.get(*name))
        .and_then(number_or_digits)
}

/// The title Seedr gave a transfer it created, when it named one.
#[must_use]
pub fn created_title(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let object = value.as_object()?;
    ["title", "name"]
        .iter()
        .find_map(|name| object.get(*name))
        .and_then(serde_json::Value::as_str)
        .map(bounded_name)
        .filter(|name| !name.is_empty())
}

/// A JSON value that is a whole number, whether it arrived as one or as its digits.
///
/// Both spellings are in the field for this provider's identifiers, and a plugin that read only
/// one of them would answer "Seedr did not name the transfer it created" for a transfer it
/// named perfectly well.
fn number_or_digits(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => seedr_common::address::parse_id(text.trim()),
        _ => None,
    }
}

/// A name, cut to whole characters at the byte bound and stripped of controls.
///
/// It is Seedr's own text, it is stored on the handle and it is compared against a folder name
/// later, so it is bounded rather than trusted.
#[must_use]
pub fn bounded_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .fold(String::new(), |mut out, character| {
            if out.len() + character.len_utf8() <= MAX_NAME_BYTES {
                out.push(character);
            }
            out
        })
        .trim()
        .to_owned()
}

/// The display name a magnet carries, for a submit whose answer named none.
///
/// Last resort, and it is worth having: the name is what a finished transfer's folder is
/// matched on, and a transfer with no name at all could never be found again.
#[must_use]
pub fn magnet_display_name(magnet: &str) -> Option<String> {
    let rest = magnet
        .strip_prefix("magnet:?")
        .or_else(|| magnet.strip_prefix("MAGNET:?"))?;
    let raw = rest
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == "dn")
        .map(|(_, value)| value)?;
    let decoded = percent_decode(&raw.replace('+', " "));
    let bounded = bounded_name(&decoded);
    (!bounded.is_empty()).then_some(bounded)
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

/// Where one finished file belongs: its bare name, and the folder path it sat on.
///
/// The transfer's own name is the root of that path, which is what turns "the magnet I pasted"
/// into the package a person expects — the same rule a crawler's `package-hint` follows, with
/// the remote job standing in for the folder. A leading segment that *is* the job name is not
/// repeated, because the walk starts at the folder Seedr named after the transfer and without
/// this every package would land in a `Name/Name` folder.
#[must_use]
pub fn place(job_name: &str, relative: &[String]) -> Option<String> {
    let job_name = job_name.trim();
    let mut place: Vec<&str> = Vec::new();
    if !job_name.is_empty() {
        place.push(job_name);
    }
    for segment in relative {
        let segment = segment.trim();
        // A segment that could leave the package is dropped rather than sanitised: the host
        // reduces a hint to a path that cannot escape anyway, and a name built out of `..`
        // would be a folder nobody asked for either way.
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains('/') {
            continue;
        }
        if place.len() == 1 && segment == job_name {
            continue;
        }
        place.push(segment);
    }
    (!place.is_empty()).then(|| place.join("/"))
}

/// How a refusal is classified, without depending on either failure representation.
#[derive(Clone, Debug, Eq, PartialEq)]
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

fn plain(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// The refusal an answer describes, or `None` when it describes none.
///
/// An answer is a refusal when its status says so **or** when its body says so, whichever comes
/// first: Seedr answers a refused call with a 200 and `{"result": false}` as readily as with a
/// status, so a status-first rule would read a refused transfer as a created one and then poll
/// an identifier that was never issued.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after_seconds: Option<u64>,
    envelope: &seedr_common::reason::ErrorEnvelope,
) -> Option<ApiFailure> {
    if (200..=299).contains(&status) && !envelope.is_refusal() {
        return None;
    }
    let mut refusal = classify(status, retry_after_seconds, envelope);
    if let Some(reason) = envelope.reason() {
        refusal.params.push(("reason", reason));
    }
    Some(refusal)
}

/// Maps one refusal onto the category the sweep acts on.
///
/// Two arms are specific to this provider rather than generic:
///
/// - **402, and a word naming the plan, are their own answer.** Seedr's documentation makes the
///   REST API a premium feature, and "your plan does not include this" is something a person
///   can act on in a way that "Seedr said no" is not.
/// - **A full account waits rather than failing.** `not_enough_space_added_to_wishlist` is
///   Seedr's own answer for a transfer that did not fit, and it says in its own name that the
///   content was remembered: a person who frees space has a job that can still run.
fn classify(
    status: u16,
    retry_after_seconds: Option<u64>,
    envelope: &seedr_common::reason::ErrorEnvelope,
) -> ApiFailure {
    let reason = envelope.reason();
    let plan_refused = matches!(
        reason.as_deref(),
        Some("premium_required" | "upgrade_required" | "not_premium")
    );
    let out_of_space = reason
        .as_deref()
        .is_some_and(|word| word.starts_with("not_enough_space"));
    match status {
        401 | 403 => plain(ErrorKind::AccountInvalid, messages::AUTH_INVALID),
        402 => plain(ErrorKind::Unsupported, messages::PLAN_REQUIRED),
        404 | 410 => plain(ErrorKind::Offline, messages::TRANSFER_GONE),
        429 => ApiFailure {
            kind: ErrorKind::RateLimited(Some(retry_after_seconds.unwrap_or(RATE_LIMIT_SECONDS))),
            code: messages::RATE_LIMITED.0,
            message: messages::RATE_LIMITED.1.to_owned(),
            params: Vec::new(),
        },
        500..=599 => plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_ERROR,
        ),
        status if (200..=299).contains(&status) && plan_refused => {
            plain(ErrorKind::Unsupported, messages::PLAN_REQUIRED)
        }
        status if (200..=299).contains(&status) && out_of_space => plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::OUT_OF_SPACE,
        ),
        status if (200..=299).contains(&status) => plain(ErrorKind::Permanent, messages::API_ERROR),
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR.0,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        },
    }
}

/// Reads a `Retry-After` stated in seconds; a date-shaped or absurd one is ignored rather than
/// guessed at, because a wrong wait is worse than the bucket's own default.
#[must_use]
pub fn retry_after_seconds(header: Option<&str>) -> Option<u64> {
    let seconds: u64 = header?.trim().parse().ok()?;
    (seconds > 0 && seconds <= MAX_RETRY_AFTER_SECONDS).then_some(seconds)
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

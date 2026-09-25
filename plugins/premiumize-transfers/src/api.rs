//! Target-independent Premiumize `transfer/*` logic: response shapes, the state machine over
//! the provider's own words, the bodies its two submit flavours take, and the place a finished
//! file belongs.
//!
//! Written against the measurement of 2026-09-22 recorded in
//! `docs/roadmap/jobs/120-23-premiumize-nimmt-auftraege-entgegen.md`:
//!
//! - **Auth flavour**: `Authorization: Bearer <api key>`, the same key the resolver sibling
//!   uses. This plugin never sees it -- every request carries the template
//!   `{{secret:premiumize_api_key}}` and the host expands it towards `www.premiumize.me` and
//!   nowhere else.
//! - **Submit**: `POST /api/transfer/create` with `src` as a URI **or** as a multipart file
//!   upload, plus optional `folder_id` and `password`. Answers `id` and `name`.
//! - **Poll**: `GET /api/transfer/list`, one row per transfer carrying `id`, `name`,
//!   `status`, `progress`, `message`, `folder_id` and `file_id`. There is no per-transfer
//!   endpoint, so the whole list is read and the row is looked up in it.
//! - **Discard**: `POST /api/transfer/delete` with `id`, and only ever from a confirmed
//!   request. `clearfinished` is deliberately never called: it is a collective call that
//!   would delete transfers rDownloader never created.
//! - **Errors**: HTTP `200` with `{"status":"error","code":"...","message":"..."}`. The body
//!   is the answer, which is why `premiumize_common::status` exists.

use premiumize_common::status::Kind;
use serde::Deserialize;

use crate::messages;

/// The vault reference the Premiumize provider keeps its API key under. The value never
/// reaches this plugin.
pub const KEY_REFERENCE: &str = "premiumize_api_key";

pub const API_BASE: &str = "https://www.premiumize.me/api";

/// `POST /api/transfer/create`.
#[derive(Default, Deserialize)]
pub struct CreatedTransfer {
    #[serde(default)]
    pub id: Option<String>,
    // `name` is deliberately not read here: what a transfer is called is the provider's, and
    // the name that matters for a package comes back from the listing at the end.
}

/// `GET /api/transfer/list`.
#[derive(Default, Deserialize)]
pub struct TransferList {
    #[serde(default)]
    pub transfers: Vec<Transfer>,
}

/// One row of `transfer/list`.
#[derive(Clone, Default, Deserialize)]
pub struct Transfer {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// A fraction from 0.0 to 1.0, not a percent.
    #[serde(default)]
    pub progress: Option<f64>,
    /// The provider's own sentence about this transfer. Read so its presence is known and
    /// never forwarded: what travels is the stable code the state maps to.
    #[serde(default)]
    pub message: Option<String>,
    /// The cloud folder a finished transfer produced. `null` while it runs.
    #[serde(default)]
    pub folder_id: Option<String>,
    /// The one file a finished transfer produced -- `null` while it runs **and** when it
    /// holds more than one file, which is why the folder is read whenever this is absent.
    #[serde(default)]
    pub file_id: Option<String>,
}

/// Where a transfer stands, in the vocabulary of `interface remote-job` rather than of
/// Premiumize.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Something is happening and nobody is needed, with a suggested wait in seconds.
    Preparing(u64),
    /// Premiumize is fetching.
    Working,
    /// The files exist. `seeding` lands here too -- see [`stage_of`].
    Ready {
        /// Whether the transfer itself is still running at the provider, which is the one
        /// thing that separates `seeding` from `finished` once the files are there.
        still_running: bool,
    },
    /// The provider ended it.
    Failed((&'static str, &'static str)),
}

/// Maps one of Premiumize's five transfer states.
///
/// Four of the five are read straight off the measurement. The fifth is the decision this
/// job was asked to make and state:
///
/// - `queued` is `Preparing`, not `Working`. Nothing is being fetched yet, and a progress bar
///   at zero percent that does not move for an hour is worse than no progress bar.
/// - `running` is `Working`, with `progress` carried across as thousandths.
/// - `finished` is `Ready`: the files are in the cloud and their addresses can be read.
/// - **`seeding` is `Ready` as well, and is neither a wait nor a failure.** At Premiumize the
///   word means the content has been fetched in full and is now being offered back to the
///   swarm; the files, the folder and the addresses exist and will not change. Waiting for
///   seeding to end would postpone a download that is ready, for an end nobody controls and
///   that may never come. So the addresses are handed over and the transfer goes on seeding
///   at the provider, undisturbed: nothing here deletes it, because `discard` is reached from
///   one confirmed request and from no other path. The one difference kept from `finished` is
///   what an empty answer means -- see [`Stage::Ready::still_running`] and `guest::ready_of`.
/// - `error` is `Failed` under a stable code; the provider's `message` is dropped.
/// - An unknown word is `Preparing` and not a failure. Premiumize has five states today and
///   a plugin that failed a transfer on a word it did not recognise would throw away a
///   transfer that was going perfectly well.
#[must_use]
pub fn stage_of(status: &str) -> Stage {
    match status {
        "queued" => Stage::Preparing(30),
        "running" => Stage::Working,
        "finished" => Stage::Ready {
            still_running: false,
        },
        "seeding" => Stage::Ready {
            still_running: true,
        },
        "error" => Stage::Failed(messages::TRANSFER_FAILED),
        _ => Stage::Preparing(60),
    }
}

/// The fraction Premiumize states, in the thousandths the contract carries.
///
/// A fraction and not a percent: `progress` runs from 0.0 to 1.0 here, where Real-Debrid's
/// runs from 0 to 100. Multiplying by the wrong factor is a progress bar that finishes at one
/// percent, which is why this has a test of its own.
#[must_use]
pub fn permille(progress: Option<f64>) -> Option<u16> {
    let progress = progress?;
    if !progress.is_finite() {
        return None;
    }
    let scaled = (progress * 1_000.0).round().clamp(0.0, 1_000.0);
    // The clamp above bounds the value into u16 range before the cast, so nothing is lost.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(scaled as u16)
}

/// Whether a provider-supplied identifier is safe to put in a query or a request path.
///
/// It comes back from Premiumize and goes out again, so it is checked rather than trusted: an
/// identifier carrying a slash or a dot segment would be a request to somewhere else on the
/// very host this plugin is allowed to reach.
#[must_use]
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// `application/x-www-form-urlencoded` body for one named field.
///
/// Percent-encodes by hand rather than pulling a URL crate into a component for one field: a
/// magnet is full of `&`, `=` and `:`, and a body that did not encode them would submit a
/// truncated address.
#[must_use]
pub fn form_body(name: &str, value: &str) -> Vec<u8> {
    let mut body = String::with_capacity(name.len() + value.len() * 3 + 1);
    body.push_str(name);
    body.push('=');
    for byte in value.as_bytes() {
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

/// The boundary a multipart upload is delimited by, derived from the payload.
///
/// Derived and not fixed, and not random either: a component has no source of randomness, and
/// a constant boundary is a body a crafted container could break out of by containing it. The
/// value is a hash of the bytes, so a container would have to contain the hash of itself.
/// [`multipart_body`] checks anyway and refuses rather than sending a broken body.
#[must_use]
pub fn boundary_for(digest: &str) -> String {
    format!("----rdownloader{digest}")
}

/// The `multipart/form-data` body `transfer/create` takes when `src` is a file.
///
/// A container arrives as bytes and has to leave as an upload rather than as a stringified
/// URI: there is no address to put in `src`, and the file name is what tells Premiumize
/// whether it is looking at a torrent, an NZB or one of the container formats it treats
/// specially. `None` when the payload contains the boundary, which cannot be worked around
/// by escaping and must not be sent.
#[must_use]
pub fn multipart_body(boundary: &str, file_name: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    let marker = boundary.as_bytes();
    if bytes.windows(marker.len()).any(|window| window == marker) {
        return None;
    }
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"src\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Some(body)
}

/// The `Content-Type` a multipart body travels under.
#[must_use]
pub fn multipart_content_type(boundary: &str) -> String {
    format!("multipart/form-data; boundary={boundary}")
}

/// Where one finished file belongs: its bare name, and the path it sat on inside the job.
///
/// The transfer's own name is the root of that path, which is what turns "the magnet I
/// pasted" into the package a person expects -- the same rule a crawler's `package-hint`
/// follows, with the remote job standing in for the folder. A leading segment that *is* the
/// transfer name is not repeated, so a package does not land in `Show.S01/Show.S01`
/// (RD-108-03).
#[must_use]
pub fn place(
    transfer_name: &str,
    folders: &[String],
    file_name: &str,
) -> (Option<String>, Option<String>) {
    let mut place = Vec::new();
    let transfer_name = transfer_name.trim();
    let mut folders: Vec<&str> = folders
        .iter()
        .map(|segment| segment.trim())
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect();
    if !transfer_name.is_empty() {
        place.push(transfer_name);
        if folders.first().is_some_and(|first| *first == transfer_name) {
            folders.remove(0);
        }
    }
    place.extend(folders);
    let hint = (!place.is_empty()).then(|| place.join("/"));
    let file_name = file_name.trim();
    let name = (!file_name.is_empty()).then(|| file_name.to_owned());
    (name, hint)
}

/// A classified refusal, in a shape that depends on neither failure representation.
///
/// Built here rather than in the guest so it can be tested on the host target -- which is
/// where the one rule that matters is checked: whatever the provider wrote in `message`, it
/// does not come out of this function. See `tests/key_canary.rs`.
#[derive(Debug, Eq, PartialEq)]
pub struct Refusal {
    pub kind: Kind,
    pub code: &'static str,
    pub message: &'static str,
    /// The provider's stable `code`, when it sent one. Never its sentence.
    pub api_code: Option<String>,
}

/// The refusal one of the provider's own answers describes.
///
/// The provider's `code` travels as the `api_code` parameter because it is stable and
/// documented. Its `message` is read -- an answer carrying no code still has to be classified
/// -- and then dropped: RD-120-23 asked for a stable code with a translation rather than a
/// sentence nobody here wrote, which is stricter than the resolver sibling and is the rule
/// for everything new.
#[must_use]
pub fn refusal(code: Option<&str>, message: Option<&str>, retry_after: Option<u64>) -> Refusal {
    let kind = premiumize_common::status::classify(code, message, retry_after);
    let (refusal_code, text) = match kind {
        Kind::AccountInvalid => messages::AUTH_INVALID,
        Kind::Unsupported => messages::SOURCE_UNSUPPORTED,
        Kind::Offline => messages::TRANSFER_GONE,
        Kind::Transient(_) => messages::SERVER_BUSY,
        Kind::RateLimited(seconds) => {
            if seconds == Some(premiumize_common::status::QUOTA_SECONDS) {
                messages::LIMIT_REACHED
            } else {
                messages::RATE_LIMITED
            }
        }
        Kind::Permanent => messages::API_ERROR,
    };
    Refusal {
        kind,
        code: refusal_code,
        message: text,
        api_code: code.filter(|code| !code.is_empty()).map(str::to_owned),
    }
}

/// The refusal an HTTP status describes when there is no envelope to read at all -- a gateway
/// page, an empty body. The status travels as a parameter rather than as a sentence.
#[must_use]
pub fn http_refusal(status: u16, retry_after: Option<u64>) -> Option<Kind> {
    premiumize_common::status::from_http_status(status, retry_after)
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

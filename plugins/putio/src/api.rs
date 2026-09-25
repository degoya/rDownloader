//! Reading what the Put.io API answered, and telling its refusals apart.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place the shapes that actually occur — a folder
//! that looks like a file until its `file_type` is read, an error document with no word in it,
//! a 200 that is not JSON at all — can be pinned down.
//!
//! Written against Put.io's published API v2:
//!
//! - **Auth flavour**: `Authorization: Bearer <token>`. This plugin never sees the token:
//!   every request carries the template `{{secret:putio_access_token}}` and the host expands it
//!   towards `api.put.io` and nowhere else.
//! - `GET /v2/files/{id}` answers `{"file": {...}}` with `name`, `size`, `file_type` and
//!   `content_type`.
//! - `GET /v2/account/info` answers `{"info": {...}}` with `username`, `account_active` and a
//!   `disk` record.
//! - **Errors**: `{"error_type": "<WORD>", "error_message": "<sentence>", "status_code": n}`.
//!   `putio_common::reason` is what decides which half of that may be repeated.
//! - **Rate limit**: Put.io answers 429 and states the window in `X-RateLimit-Reset`, a Unix
//!   timestamp rather than a duration, so turning it into a wait needs the host's clock.

use serde::Deserialize;

pub use plugin_common::FailureKind;
use putio_common::reason::ErrorEnvelope;

use crate::messages;

/// The vault reference the Put.io provider keeps its access token under. The value never
/// reaches this plugin.
pub const TOKEN_REFERENCE: &str = "putio_access_token";

/// The `content_type` Put.io gives a folder. `file_type` says `FOLDER` for the same thing, and
/// both are read: a folder that arrives under only one of them is still a folder.
pub const FOLDER_CONTENT_TYPE: &str = "application/x-directory";

/// What one file is, as `GET /v2/files/{id}` states it.
#[derive(Debug, Default, Deserialize)]
pub struct FileRecord {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub file_type: Option<String>,
    /// Put.io's own CRC32 of the stored file, lower-case hex. Reported so the transfer can be
    /// verified against what the provider holds rather than against nothing.
    #[serde(default)]
    pub crc32: Option<String>,
}

impl FileRecord {
    /// Whether this record is a folder rather than something with bytes in it.
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.file_type.as_deref() == Some("FOLDER")
            || self.content_type.as_deref() == Some(FOLDER_CONTENT_TYPE)
    }

    /// The checksum to verify the transfer against, when Put.io states a usable one.
    ///
    /// A CRC32 of all zeroes or of the wrong length is not a checksum, it is a field Put.io
    /// has not filled in; checking against it would fail every download of a file the provider
    /// simply has not hashed.
    #[must_use]
    pub fn checksum(&self) -> Option<(String, String)> {
        let crc = self.crc32.as_deref()?.trim();
        let usable = crc.len() == 8
            && crc.bytes().all(|byte| byte.is_ascii_hexdigit())
            && crc.bytes().any(|byte| byte != b'0');
        usable.then(|| ("crc32".to_owned(), crc.to_ascii_lowercase()))
    }
}

/// `GET /v2/files/{id}`.
#[derive(Debug, Default, Deserialize)]
pub struct FileResponse {
    #[serde(default)]
    pub file: Option<FileRecord>,
}

/// The `disk` record of `GET /v2/account/info`.
#[derive(Debug, Default, Deserialize)]
pub struct Disk {
    #[serde(default)]
    pub avail: Option<u64>,
}

/// `GET /v2/account/info`.
#[derive(Debug, Default, Deserialize)]
pub struct AccountInfo {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub mail: Option<String>,
    #[serde(default)]
    pub account_active: Option<bool>,
    #[serde(default)]
    pub disk: Option<Disk>,
}

/// `GET /v2/account/info`, the envelope around it.
#[derive(Debug, Default, Deserialize)]
pub struct AccountResponse {
    #[serde(default)]
    pub info: Option<AccountInfo>,
}

/// A classified refusal: what it is, what the interface calls it, and what may be repeated.
#[derive(Debug, Eq, PartialEq)]
pub struct ApiFailure {
    pub kind: FailureKind,
    pub code: &'static str,
    pub message: String,
    /// The sanitised `error_type`, when Put.io stated one.
    pub reason: Option<String>,
}

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// API-shaped providers in this tree settled on.
const BUSY_SECONDS: u64 = 300;

/// The wait a 429 gets when Put.io states no usable reset. A minute: refused requests count
/// towards the very cap that refused them, so asking again at once only extends it.
const RATE_LIMIT_SECONDS: u64 = 60;

/// The longest wait a `X-RateLimit-Reset` is believed for. The header is an absolute Unix
/// timestamp, so a clock that disagrees with Put.io's — or a header from a proxy — could
/// otherwise park a job for days.
const MAX_RATE_LIMIT_SECONDS: u64 = 3600;

fn failure(kind: FailureKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        reason: None,
    }
}

/// The refusal an answer describes, or `None` when it describes none.
///
/// An answer is a refusal when its status says so or when it carries an error document,
/// whichever comes first; a 2xx carrying one is still a refusal.
///
/// `reset_in_seconds` is how long the caller worked out the rate-limit window still has to
/// run — see [`rate_limit_wait`], which is where the clock is needed.
#[must_use]
pub fn failure_from(
    status: u16,
    reset_in_seconds: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if (200..=299).contains(&status) && !envelope.is_refusal() {
        return None;
    }
    let mut refusal = classify(status, reset_in_seconds, envelope);
    refusal.reason = envelope.kind();
    Some(refusal)
}

/// Maps one refusal onto the category the scheduler acts on.
///
/// The status decides, because Put.io's statuses are the part that is documented and stable.
/// `error_type` is read for one thing only — a 403 that is really an expired token, which
/// Put.io does send — because the difference between "sign in again" and "your account may not
/// do this" is the difference between a person acting and a person waiting.
fn classify(status: u16, reset_in_seconds: Option<u64>, envelope: &ErrorEnvelope) -> ApiFailure {
    let kind = envelope.kind();
    let token_refused = matches!(
        kind.as_deref(),
        Some("INVALID_TOKEN" | "INVALID_GRANT" | "UNAUTHORIZED")
    );
    match status {
        401 => failure(FailureKind::AccountInvalid, messages::AUTH_INVALID),
        403 if token_refused => failure(FailureKind::AccountInvalid, messages::AUTH_INVALID),
        403 => failure(FailureKind::Unsupported, messages::NOT_PERMITTED),
        404 | 410 => failure(FailureKind::Permanent, messages::FILE_NOT_FOUND),
        429 => failure(
            FailureKind::RateLimited(Some(reset_in_seconds.unwrap_or(RATE_LIMIT_SECONDS))),
            messages::RATE_LIMITED,
        ),
        500..=599 => failure(
            FailureKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_ERROR,
        ),
        // An error document with a 2xx or an unremarkable 4xx: Put.io said no and this build
        // has no bucket for it. The word travels as `reason`; the sentence does not.
        status if (200..=299).contains(&status) => {
            failure(FailureKind::Permanent, messages::API_ERROR)
        }
        other => ApiFailure {
            kind: FailureKind::Permanent,
            code: messages::HTTP_ERROR.0,
            message: messages::http_error(other),
            reason: None,
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

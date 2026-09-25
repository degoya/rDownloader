//! Target-independent Pixeldrain logic: the addresses, the response shapes and the failure
//! classification. Nothing here touches a host, so all of it is testable without a runtime.
//!
//! Written against the provider's published API, <https://pixeldrain.com/api>, as measured on
//! 2026-09-22 (RD-120-07):
//!
//! - **Metadata**: `GET /api/file/{id}/info` answers with the file's `id`, `name`, `size`,
//!   `mime_type`, `hash_sha256` and an `availability` field.
//! - **Bytes**: `GET /api/file/{id}` serves the file itself and honours byte ranges, which is
//!   what lets the existing HTTP engine resume it. `?download` asks for a `Content-Disposition`
//!   attachment rather than an inline rendering. The address carries no signature and no
//!   deadline: it is the identifier, so it is as durable as the file.
//! - **Quotas**: `GET /api/misc/rate_limits` reports `download_limit`, `download_limit_used`,
//!   `transfer_limit`, `transfer_limit_used`, `speed_limit` and `server_overload` **before** a
//!   download starts. That is the whole reason it is consulted here: the bytes are fetched by
//!   the transfer engine, not by this plugin, so a limit that is already spent has to be turned
//!   into a scheduled wait here or it arrives as a 429 in the middle of a transfer instead.
//! - **Refusals**: `{"success":false,"value":"<token>","message":"<prose>"}`. The `value` field
//!   is the stable machine-readable code; the `message` is prose and is never forwarded.
//!
//! **What is measured and what is read.** The field *names* above were seen on live answers.
//! What "used" means against "limit" was not measured as a number, so the reading applied here
//! is stated rather than assumed: a limit of zero means "no limit stated" and is ignored, and a
//! non-zero limit counts as reached only when the used figure has caught up with it. A wrong
//! reading in that direction costs a wait that was not necessary; the opposite would spend the
//! person's allowance on a transfer that cannot finish.
//!
//! - **Authentication** (measured 2026-09-23, RD-120-38): an API key is an HTTP Basic password
//!   under an empty user name. `GET /api/user` without one answers `401` with the token
//!   `authentication_required`; with an unknown key it, and every other endpoint, answers `401`
//!   `authentication_failed` -- so a wrong key does not fall back to the free route, it stops.
//!
//! **Nothing here has been run against a premium Pixeldrain subscription.** The free route is
//! what the feasibility measurement covered, and the refusals above are all that was measured
//! of the keyed one. The shape of a *successful* `/api/user` answer was read from the
//! provider's own open-source web client, not seen on the wire; see [`User`].

use serde::Deserialize;

use crate::messages;

pub const HOST: &str = "pixeldrain.com";
pub const API_BASE: &str = "https://pixeldrain.com/api";

/// How long a provider-side overload is waited out.
const OVERLOAD_SECONDS: u64 = 300;

/// How long a spent per-IP allowance is waited out when the answer names no better figure.
/// Pixeldrain states its download allowance per day, so an hour is a retry rather than a
/// spin -- short enough that a limit lifted early is noticed, long enough not to hammer.
const QUOTA_SECONDS: u64 = 3600;

/// How long a concurrency refusal is waited out. Unlike the quota this clears as soon as
/// another transfer finishes, so it is minutes rather than an hour.
const CONCURRENCY_SECONDS: u64 = 300;

/// `GET /api/file/{id}/info`.
///
/// Every field is optional on purpose: the answer grew fields over time and a missing one is
/// not a reason to refuse a file whose name and size are right there. The answer carries more
/// than this -- `id`, `mime_type`, view counters, dates -- and what is not read is not declared:
/// a field nobody consults is a claim about the provider that nothing ever checks.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FileInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Hex SHA-256 of the stored bytes, when the provider states one.
    #[serde(default)]
    pub hash_sha256: Option<String>,
    /// Empty when the file may be fetched; a token naming the obstacle otherwise.
    #[serde(default)]
    pub availability: Option<String>,
}

/// `GET /api/misc/rate_limits`.
///
/// The answer also carries `speed_limit`, which is not read here: this plugin does not move the
/// bytes, so a throttle it could neither observe nor honour is the transfer engine's business.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct RateLimits {
    #[serde(default)]
    pub download_limit: u64,
    #[serde(default)]
    pub download_limit_used: u64,
    #[serde(default)]
    pub transfer_limit: u64,
    #[serde(default)]
    pub transfer_limit_used: u64,
    #[serde(default)]
    pub server_overload: bool,
}

/// The refusal envelope every endpoint shares.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub success: Option<bool>,
    /// The stable code. This is the only part of a refusal that travels.
    #[serde(default)]
    pub value: Option<String>,
}

/// How a refusal is classified, without depending on either failure representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    RateLimited(Option<u64>),
    IpBlocked(Option<u64>),
    AuthRequired,
    /// The stored credential itself is refused: the account needs a new one.
    AccountInvalid,
    Unsupported,
}

/// A classified refusal.
#[derive(Clone, Debug)]
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

/// What is left of the provider's word once everything that is not code-shaped is gone.
///
/// `value` is documented as a machine-readable token, but it arrives over the network and is
/// shown to a person, so it is held to that shape rather than trusted to have it. Anything
/// longer or carrying punctuation is dropped whole rather than filtered: a value that echoed
/// part of a request would keep whatever survived the filter.
#[must_use]
pub fn sanitize_value(value: &str) -> Option<String> {
    let value = value.trim();
    let code_shaped = !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    code_shaped.then(|| value.to_ascii_lowercase())
}

/// Classifies one of Pixeldrain's documented `value` tokens.
///
/// Each group keeps its own code rather than collapsing into one "try later", because each asks
/// something different of the person: wait, wait longer, buy a subscription, sign in, give up on
/// this file, or report a token nobody here has seen before.
#[must_use]
pub fn classify_value(value: &str) -> ApiFailure {
    match sanitize_value(value).as_deref() {
        Some("not_found" | "file_not_found" | "list_not_found") => {
            plain(ErrorKind::Offline, messages::FILE_NOT_FOUND)
        }
        // The file is there and the provider will not serve it. Permanent: no wait and no other
        // address changes a moderation verdict.
        Some("virus_detected_abuse" | "file_blocked" | "abuse") => {
            plain(ErrorKind::Permanent, messages::FILE_BLOCKED)
        }
        // This IP has spent its share. Pixeldrain's own wording, and the state its
        // `robots.txt`-permissive service is entitled to enforce.
        Some("ip_rate_limit_reached" | "rate_limited" | "too_many_requests") => plain(
            ErrorKind::IpBlocked(Some(QUOTA_SECONDS)),
            messages::IP_RATE_LIMITED,
        ),
        // The account's or the file's transfer volume for the period is gone. A premium
        // subscription on either end is what lifts it, which is what the translation says.
        Some("transfer_limit_exceeded" | "download_limit_reached") => plain(
            ErrorKind::RateLimited(Some(QUOTA_SECONDS)),
            messages::TRANSFER_LIMIT,
        ),
        Some("max_concurrent_downloads") => plain(
            ErrorKind::RateLimited(Some(CONCURRENCY_SECONDS)),
            messages::TOO_MANY_DOWNLOADS,
        ),
        // A captcha stands in front of this file. This plugin declares no `captcha` capability
        // -- the flow behind it was never measured -- so it says so instead of guessing at it.
        Some("file_rate_limited_captcha_required" | "captcha_required") => {
            plain(ErrorKind::Unsupported, messages::CAPTCHA_REQUIRED)
        }
        Some("authentication_required" | "unauthorized" | "permission_denied") => {
            plain(ErrorKind::AuthRequired, messages::ACCOUNT_REQUIRED)
        }
        // A stored key the provider does not accept -- invalid, revoked or expired, in its own
        // words (measured 2026-09-23). The account is wrong, not the file, and every request
        // with that key will answer the same until somebody enters a new one.
        Some("authentication_failed") => {
            plain(ErrorKind::AccountInvalid, messages::API_KEY_INVALID)
        }
        Some("internal" | "internal_error") => plain(
            ErrorKind::Transient(Some(OVERLOAD_SECONDS)),
            messages::SERVER_ERROR,
        ),
        // Code-shaped and unknown to this build. The token travels as a parameter so the
        // person can quote it in a report; the provider's sentence does not.
        Some(token) => with_param(
            ErrorKind::Permanent,
            messages::API_ERROR,
            "api_code",
            token.to_owned(),
        ),
        // Not code-shaped, so there is nothing to branch on and nothing safe to show. Transient
        // rather than permanent: an unreadable answer is at least as likely to be a passing
        // outage as a verdict about this file.
        None => plain(
            ErrorKind::Transient(Some(OVERLOAD_SECONDS)),
            messages::INVALID_RESPONSE,
        ),
    }
}

/// The refusal an answer carries, or an empty envelope when it carries none.
///
/// **Only a JSON object can be a refusal.** Serde reads a struct out of a *sequence* as readily
/// as out of a map, taking elements in field order, so an array answer read straight into
/// [`ErrorEnvelope`] would invent `success`/`value` out of its first two elements.
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
/// The document is read before the status, and that order matters in both directions:
/// Pixeldrain's refusals carry their own HTTP status *and* the token, and the token is the finer
/// answer of the two -- a 404 alone cannot tell a deleted file from a blocked one. A status with
/// no readable document falls through to [`ensure_http_status`].
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if envelope.success == Some(false)
        && let Some(value) = envelope.value.as_deref()
    {
        return Some(classify_value(value));
    }
    ensure_http_status(status, retry_after).err()
}

/// Maps an HTTP status no document explains.
///
/// # Errors
///
/// The classified refusal, for every status that is not a 2xx.
pub fn ensure_http_status(status: u16, retry_after: Option<u64>) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(plain(ErrorKind::AuthRequired, messages::ACCOUNT_REQUIRED)),
        404 | 410 => Err(plain(ErrorKind::Offline, messages::FILE_NOT_FOUND)),
        429 => Err(plain(
            ErrorKind::IpBlocked(Some(retry_after.unwrap_or(QUOTA_SECONDS))),
            messages::IP_RATE_LIMITED,
        )),
        500..=599 => Err(plain(
            ErrorKind::Transient(Some(retry_after.unwrap_or(OVERLOAD_SECONDS))),
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

/// What `availability` says about a file that exists.
///
/// Pixeldrain distinguishes more states than this project's `link-status` enum can hold --
/// `online | offline | unknown` and nothing else (RD-120-36). The collapse is deliberate and
/// stated here rather than spread over the call sites: a file behind a captcha or a spent
/// allowance **exists**, so a link check reports it online and the download attempt is what
/// carries the obstacle's own code. Only a moderation block is reported as offline, because for
/// this application it is: no wait and no retry will ever produce the bytes.
#[must_use]
pub fn availability_failure(info: &FileInfo) -> Option<ApiFailure> {
    let value = info.availability.as_deref().map(str::trim).unwrap_or("");
    (!value.is_empty()).then(|| classify_value(value))
}

/// Whether a file that exists should nevertheless be reported as gone.
#[must_use]
pub fn availability_is_offline(info: &FileInfo) -> bool {
    availability_failure(info).is_some_and(|failure| failure.code == messages::FILE_BLOCKED.0)
}

/// The quota refusal the pre-download check found, or `None` when nothing stands in the way.
///
/// Consulted before a download is handed to the queue, because the transfer engine fetches the
/// bytes and this plugin would never see the 429 that a spent allowance produces otherwise.
#[must_use]
pub fn quota_failure(limits: &RateLimits) -> Option<ApiFailure> {
    if limits.server_overload {
        return Some(plain(
            ErrorKind::Transient(Some(OVERLOAD_SECONDS)),
            messages::SERVER_OVERLOADED,
        ));
    }
    if reached(limits.transfer_limit, limits.transfer_limit_used) {
        return Some(plain(
            ErrorKind::RateLimited(Some(QUOTA_SECONDS)),
            messages::TRANSFER_LIMIT,
        ));
    }
    if reached(limits.download_limit, limits.download_limit_used) {
        return Some(plain(
            ErrorKind::IpBlocked(Some(QUOTA_SECONDS)),
            messages::IP_RATE_LIMITED,
        ));
    }
    None
}

/// A limit of zero states no limit; anything else is reached once the used figure catches up.
fn reached(limit: u64, used: u64) -> bool {
    limit > 0 && used >= limit
}

/// `GET /api/user`, read for one thing: whether the account pays.
///
/// **Read, not measured.** Without a key the endpoint answers 401, and no key was available to
/// see a successful answer. The field comes from the provider's own web client
/// (`svelte/src/lib/PixeldrainAPI.ts`, type `User`), whose account page treats an empty
/// `subscription.id` as the free tier and `patreon`/`prepaid` as paying. Everything is optional,
/// so an answer that lacks the field is a valid account that is not claimed to be premium.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
    #[serde(default)]
    pub subscription: Option<Subscription>,
}

/// The one field of `User.subscription` read here.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Subscription {
    #[serde(default)]
    pub id: Option<String>,
}

/// Whether the account behind a key has a paid subscription.
#[must_use]
pub fn user_is_premium(user: &User) -> bool {
    user.subscription
        .as_ref()
        .and_then(|subscription| subscription.id.as_deref())
        .is_some_and(|id| !id.trim().is_empty())
}

/// The address the metadata of one file lives at.
#[must_use]
pub fn info_url(id: &str) -> String {
    format!("{API_BASE}/file/{id}/info")
}

/// The address the bytes of one file live at.
///
/// `?download` asks for a `Content-Disposition` attachment. Deliberately no signature, no token
/// and no expiry: this is the identifier, so the address stays good for as long as the file
/// does and a job that waits an hour in the queue still has a working one when its turn comes.
#[must_use]
pub fn download_url(id: &str) -> String {
    format!("{API_BASE}/file/{id}?download")
}

/// The address `GET /api/user` lives at: the account an API key belongs to.
#[must_use]
pub fn user_url() -> String {
    format!("{API_BASE}/user")
}

/// The address `GET /api/misc/rate_limits` lives at.
#[must_use]
pub fn rate_limits_url() -> String {
    format!("{API_BASE}/misc/rate_limits")
}

/// The file identifier in a Pixeldrain address, or `None` when this plugin does not claim it.
///
/// Three shapes are claimed, and all three name one file: the share page `/u/{id}` and the API's
/// own `/api/file/{id}` and `/api/file/{id}/info`. Nothing else, and deliberately -- a claimed
/// address is a promise, and a shape nobody measured turns a plain HTTP download into a hoster
/// error when the promise does not hold. The list address `/l/{id}` is left out for a different
/// reason: it names several files, which is `plugins/pixeldrain-crawler/`'s job, and a resolver
/// answers with exactly one download.
#[must_use]
pub fn file_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !is_pixeldrain_host(parsed.host_str()?) {
        return None;
    }
    let segments: Vec<&str> = parsed
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let id = match segments.as_slice() {
        ["u", id] => *id,
        ["api", "file", id] | ["api", "file", id, "info"] => *id,
        _ => return None,
    };
    valid_id(id).then(|| (*id).to_owned())
}

/// Whether a host is one this plugin serves, `www.` or not.
#[must_use]
pub fn is_pixeldrain_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == HOST || host == format!("www.{HOST}")
}

/// Whether an address is one the queue may be handed for these bytes.
///
/// Applied to the address this plugin produces, not to the one it was given. It produces that
/// address itself today, so the check reads as belt and braces -- and it is exactly the check
/// that would catch a future version following a redirect off the provider's own hosts.
#[must_use]
pub fn is_download_target(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            && parsed.host_str().is_some_and(|host| {
                let host = host.trim_end_matches('.').to_ascii_lowercase();
                is_pixeldrain_host(&host) || host.ends_with(&format!(".{HOST}"))
            })
    })
}

/// Pixeldrain identifiers are short base-62 tokens. Bounded here so a path that merely looks
/// like one -- an escaped segment, a sentence -- is not claimed and then failed at the provider.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The file name, trimmed, or `None` when the provider stated none worth using.
///
/// A name arrives from the provider and becomes a path component downstream, so an empty or
/// whitespace-only one is dropped here rather than turned into a file called nothing. Path
/// separators and traversal are the file layer's to reject, and it does; what this must not do
/// is invent a name the provider never gave.
#[must_use]
pub fn file_name(info: &FileInfo) -> Option<String> {
    info.name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The checksum the provider states, as `(algorithm, value)`.
///
/// Held to the shape of a SHA-256 digest: 64 hex characters. A field carrying anything else is
/// dropped, because a checksum that does not verify fails every download of a perfectly good
/// file.
#[must_use]
pub fn checksum(info: &FileInfo) -> Option<(String, String)> {
    let hash = info.hash_sha256.as_deref()?.trim();
    let hex = hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
    hex.then(|| ("sha256".to_owned(), hash.to_ascii_lowercase()))
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

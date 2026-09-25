//! Target-independent Real-Debrid REST 1.0 logic: request bodies, response shapes and error
//! classification. Shared verbatim by the native (`native.rs`) and WebAssembly (`guest.rs`)
//! adapters so both report byte-identical failure codes and messages; neither `rd-core` nor
//! `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`, which are
//! available on every target.
//!
//! Written against the published API document at <https://api.real-debrid.com/>:
//!
//! - **Auth flavour**: `Authorization: Bearer <token>`. The token is what the OAuth2 device
//!   flow in `plugins/realdebrid-auth/` produced, and it expires — which is the whole reason
//!   that sibling is an `oauth` plugin and not an `auth` one. This plugin never sees it: every
//!   request carries the template `{{secret:realdebrid_access_token}}` and the host expands it
//!   on the way out, towards `api.real-debrid.com` and nowhere else.
//! - **Unrestriction**: `POST /unrestrict/link` with a form body carrying `link`. The answer's
//!   `download` field is the generated address; `link` is the original one echoed back, and
//!   confusing the two would queue the hoster page instead of the file.
//! - **Link check**: `POST /unrestrict/check` with `link`, one address per call. It takes no
//!   token, so a check works before an account is signed in — but the plugin still sends one
//!   request per link, so the batch is bounded (see [`CHECK_LIMIT`]).
//! - **Catalogue**: `GET /hosts/domains` answers a bare JSON array of domains and needs no
//!   token either.
//! - **Account**: `GET /user` answers `type` (`premium`/`free`) and `premium`, the seconds of
//!   premium time left. There is no remaining-traffic figure in bytes anywhere in the API, so
//!   `traffic_left` stays `None` rather than being invented from `points`, which counts
//!   loyalty points and not bytes.
//! - **Errors**: `{"error": "<sentence>", "error_code": <number>}`. The number is stable and
//!   documented, the sentence is not; [`classify_error`] reads the number and drops the
//!   sentence. See `messages.rs` for why.
//! - **Rate limit**: the API is capped at 250 requests a minute for everybody, and *refused*
//!   requests count towards the cap. So 429 and `error_code` 5/34 are waits with a floor, and
//!   `Retry-After` is honoured when it is there.

use serde::Deserialize;
use url::form_urlencoded;

use crate::messages;

/// `rd-provider-registry`'s `realdebrid` row: `secret_reference`. The OAuth device flow in
/// `plugins/realdebrid-auth/` writes the access token here through `store-oauth-token`.
pub(crate) const TOKEN_REFERENCE: &str = "realdebrid_access_token";

pub(crate) const API_BASE: &str = "https://api.real-debrid.com/rest/1.0";

/// How many addresses one `check` invocation will ask about.
///
/// `unrestrict/check` answers about one link per request, so a batch of a thousand pasted
/// links would be a thousand requests against an account capped at 250 a minute — the plugin
/// would rate-limit the very account it is checking for. Everything past this many comes back
/// `Unknown`, which is the honest answer: not checked, rather than not there.
pub(crate) const CHECK_LIMIT: usize = 40;

/// Real-Debrid is a multihoster: it claims any http(s) URL (mirrors `premiumize`/`alldebrid`).
///
/// Note what this excludes. A `magnet:` address parses perfectly well as a URL and Real-Debrid
/// does accept magnets — through `torrents/addMagnet`, which starts something that runs for
/// minutes and has to have its files chosen before any address exists. That is not a `resolve`,
/// so this plugin does not claim it and does not pretend it can.
pub(crate) fn matches(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// `application/x-www-form-urlencoded` body for `POST /unrestrict/link` and
/// `POST /unrestrict/check`.
pub(crate) fn link_body(link: &str) -> Vec<u8> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("link", link);
    serializer.finish().into_bytes()
}

/// Validates the raw address `unrestrict/link`'s `download` field carries. Shared so a
/// malformed URL from the API produces the exact same `realdebrid.invalid_url` failure on both
/// adapters instead of one erroring and the other forwarding an address that fails deeper in
/// the pipeline.
pub(crate) fn parse_download_url(raw: &str) -> Result<url::Url, ApiFailure> {
    url::Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// The failure envelope every endpoint answers a refusal with.
#[derive(Default, Deserialize)]
pub(crate) struct ErrorEnvelope {
    /// The provider's own sentence. Read so its presence can be detected and **never**
    /// forwarded: see the module doc of `messages.rs`.
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) error_code: Option<i64>,
}

/// `POST /unrestrict/link`.
#[derive(Default, Deserialize)]
pub(crate) struct UnrestrictedLink {
    /// The generated address. `link` beside it is the original one echoed back.
    #[serde(default)]
    pub(crate) download: Option<String>,
    #[serde(default)]
    pub(crate) filename: Option<String>,
    #[serde(default)]
    pub(crate) filesize: Option<u64>,
    // `host` and `crc` are deliberately not read. `crc` is stated as `0` or `1` -- a flag
    // saying whether a check ran, not a digest -- and forwarding it as a checksum would make
    // every download fail its own verification.
}

/// `POST /unrestrict/check`.
#[derive(Default, Deserialize)]
pub(crate) struct CheckedLink {
    #[serde(default)]
    pub(crate) filename: Option<String>,
    #[serde(default)]
    pub(crate) filesize: Option<u64>,
    // `supported` is deliberately not read: a file that is there on a hoster Real-Debrid
    // does not cover is still there, so it can decide nothing about a link's status.
}

/// `GET /user`.
#[derive(Default, Deserialize)]
pub(crate) struct UserInfo {
    #[serde(default)]
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) email: Option<String>,
    /// `premium` or `free`.
    #[serde(default, rename = "type")]
    pub(crate) account_type: Option<String>,
    /// Seconds of premium time left. `0` on a free account.
    #[serde(default)]
    pub(crate) premium: Option<i64>,
}

/// Lower-cases, deduplicates and sorts the `hosts/domains` array (mirrors
/// `alldebrid::merge_hosters`/`premiumize::services::merge_hosters`).
pub(crate) fn merge_hosters(domains: Vec<String>) -> Vec<String> {
    let mut hosters: Vec<String> = domains
        .into_iter()
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect();
    hosters.sort();
    hosters.dedup();
    hosters
}

/// Premium only when the account says so *and* still has time on it. An expired premium
/// account keeps `type: "premium"` for a while, and treating that as premium would tell the
/// person their plan is fine while every unrestriction fails.
pub(crate) fn is_premium(account_type: Option<&str>, premium_seconds: Option<i64>) -> bool {
    account_type.is_some_and(|value| value.eq_ignore_ascii_case("premium"))
        && premium_seconds.is_none_or(|seconds| seconds > 0)
}

/// The name the account label shows: the Real-Debrid username, falling back to the account
/// e-mail; `None` when the API states neither, and the label then says nothing.
pub(crate) fn account_name<'a>(
    username: Option<&'a str>,
    email: Option<&'a str>,
) -> Option<&'a str> {
    username
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| email.map(str::trim).filter(|value| !value.is_empty()))
}

/// Failure classification independent of the native (`rd_core::Failure`) and WASM
/// (WIT-generated `Failure`) representations; both adapters convert this into their own type.
#[derive(Debug)]
pub(crate) struct ApiFailure {
    pub(crate) kind: ErrorKind,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) params: Vec<(&'static str, String)>,
}

/// Mirrors `rd_core::FailureKind` / the WIT `failure-kind` variant, without depending on either.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    #[allow(dead_code)] // Real-Debrid's JSON API never challenges with a captcha.
    NeedsCaptcha,
    Unsupported,
    IpBlocked(Option<u64>),
}

/// How long a hoster-side or provider-side outage is waited out before the link is tried
/// again. Five minutes, the same figure the other multihoster plugins settled on.
const BUSY_SECONDS: u64 = 300;

/// How long an exhausted quota is waited out. Real-Debrid's traffic and fair-use windows are
/// stated in days, so any figure here is a compromise; an hour is short enough that a queue
/// recovers on its own and long enough not to spend the request budget asking.
const QUOTA_SECONDS: u64 = 3600;

/// Attaches the provider's documented `error_code` number so the interface can tell which one
/// triggered a shared bucket. The number is data, unlike the sentence beside it.
fn coded_with_api_code(
    kind: ErrorKind,
    (code, message): (&'static str, &str),
    api_code: i64,
) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: vec![("api_code", api_code.to_string())],
    }
}

/// Classifies a documented `error_code`.
///
/// The buckets follow <https://api.real-debrid.com/> and are grouped by what the scheduler can
/// usefully do about them, which is not always what the wording suggests:
///
/// - 20 ("hoster not available for free users") is `Unsupported` rather than an account error:
///   the account is fine, this hoster simply is not covered by its plan, and retrying it on a
///   timer would achieve nothing.
/// - 22 ("IP address not allowed") is `IpBlocked` rather than `AccountInvalid`: the credential
///   is good, the address is not, and blocking the whole account for it would stop every other
///   download that is going perfectly well.
/// - 5 ("slow down") and 34 ("too many requests") are the same wait, because the API counts
///   refused requests towards the very cap that refused them.
fn classify_error(api_code: i64, retry_after: Option<u64>) -> ApiFailure {
    match api_code {
        8 | 9 | 12 | 13 | 14 | 15 => {
            coded_with_api_code(ErrorKind::AccountInvalid, messages::AUTH_INVALID, api_code)
        }
        10 | 11 => coded_with_api_code(ErrorKind::AuthRequired, messages::TWO_FACTOR, api_code),
        7 | 24 | 35 => coded_with_api_code(ErrorKind::Offline, messages::FILE_OFFLINE, api_code),
        16 | 20 => {
            coded_with_api_code(ErrorKind::Unsupported, messages::HOST_UNSUPPORTED, api_code)
        }
        6 | 17 | 19 | 21 | 25 => coded_with_api_code(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_BUSY,
            api_code,
        ),
        18 | 23 | 36 => coded_with_api_code(
            ErrorKind::RateLimited(Some(QUOTA_SECONDS)),
            messages::LIMIT_REACHED,
            api_code,
        ),
        22 => coded_with_api_code(
            ErrorKind::IpBlocked(None),
            messages::IP_NOT_ALLOWED,
            api_code,
        ),
        5 | 34 => coded_with_api_code(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
            api_code,
        ),
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR,
            message: messages::api_error(other),
            params: vec![("api_code", other.to_string())],
        },
    }
}

/// The failure an answer describes, or `None` when it describes none.
///
/// An answer is a failure when it carries an `error_code`, whatever its HTTP status; a 2xx
/// carrying one is still a refusal, and a 4xx carrying none is classified by its status alone.
pub(crate) fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if let Some(api_code) = envelope.error_code {
        return Some(classify_error(api_code, retry_after));
    }
    // A sentence with no number is still a refusal — it is just one the document does not
    // name, so it goes to the generic bucket by status rather than being read.
    if envelope.error.is_some() || !(200..=299).contains(&status) {
        return ensure_http_status(status, retry_after).err();
    }
    None
}

/// Maps an HTTP status no `error_code` explains.
pub(crate) fn ensure_http_status(status: u16, retry_after: Option<u64>) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(plain(ErrorKind::AccountInvalid, messages::AUTH_INVALID)),
        404 | 410 | 451 => Err(plain(ErrorKind::Offline, messages::FILE_OFFLINE)),
        429 => Err(plain(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
        )),
        500..=599 => Err(plain(ErrorKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
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

/// Reads a `Retry-After` header stated in seconds. A date-shaped one is ignored rather than
/// guessed at: a wrong wait is worse than the bucket's own default.
pub(crate) fn retry_after_seconds(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

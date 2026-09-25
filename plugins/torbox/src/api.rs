//! Target-independent TorBox resolver logic: the address shape this plugin claims, the answer
//! shapes and the failure classification.
//!
//! Only two endpoints are reached from here. `GET /user/me` says what the account is worth,
//! and `GET /{kind}/requestdl` turns a finished job's file into the short-lived address the
//! bytes come from. Everything else TorBox offers belongs to `plugins/torbox-jobs/`.

use serde::Deserialize;

use crate::messages;

/// The vault reference the TorBox provider keeps its API key under. The value never reaches
/// this plugin.
pub const TOKEN_REFERENCE: &str = "torbox_api_key";

pub const API_BASE: &str = "https://api.torbox.app/v1/api";

/// The one host this plugin may reach, and therefore the one host an address it claims may
/// name.
pub const API_HOST: &str = "api.torbox.app";

/// How many addresses one `check` call asks about before the rest come back `Unknown`.
///
/// One request per address against an account-wide budget shared with the polling of every
/// running job, so a batch of five hundred pasted links must not become five hundred requests.
pub const CHECK_LIMIT: usize = 40;

/// The three shapes of `requestdl` address, and the list endpoint each belongs to.
///
/// A table rather than three functions: what a caller needs from an address is always the same
/// three things, and reading them out of one row is what keeps the `web_id`/`webdl_id`
/// asymmetry from being invented twice.
pub const KINDS: &[Kind] = &[
    Kind {
        request_path: "/torrents/requestdl",
        list_path: "/torrents/mylist",
        id_field: "torrent_id",
    },
    Kind {
        request_path: "/usenet/requestdl",
        list_path: "/usenet/mylist",
        id_field: "usenet_id",
    },
    Kind {
        request_path: "/webdl/requestdl",
        list_path: "/webdl/mylist",
        id_field: "web_id",
    },
];

/// One of TorBox's three job kinds, as this plugin needs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Kind {
    pub request_path: &'static str,
    pub list_path: &'static str,
    pub id_field: &'static str,
}

/// What a `requestdl` address names: which kind of job, which job, which file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ticket {
    pub kind: &'static Kind,
    pub job_id: String,
    pub file_id: String,
}

/// Reads a `requestdl` address, or answers `None` when it is not one of ours.
///
/// Parsed rather than forwarded. The address arrives from the host's own candidate row, which
/// a person can edit, so what goes back out to TorBox is rebuilt from the two identifiers this
/// function recognised and nothing else -- a parameter somebody added on the way through is
/// dropped rather than carried into a request that also carries the account's key.
#[must_use]
pub fn read_ticket(url: &str) -> Option<Ticket> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(API_HOST) {
        return None;
    }
    let path = parsed.path();
    let kind = KINDS
        .iter()
        .find(|kind| path == format!("/v1/api{}", kind.request_path))?;
    let mut job_id = None;
    let mut file_id = None;
    for (name, value) in parsed.query_pairs() {
        if name == kind.id_field {
            job_id = Some(value.into_owned());
        } else if name == "file_id" {
            file_id = Some(value.into_owned());
        }
    }
    let job_id = job_id.filter(|id| is_safe_id(id))?;
    let file_id = file_id.filter(|id| is_safe_id(id))?;
    Some(Ticket {
        kind,
        job_id,
        file_id,
    })
}

/// Whether an identifier read out of an address is safe to put back into a request.
#[must_use]
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Whether this plugin claims an address at all.
#[must_use]
pub fn matches(url: &str) -> bool {
    read_ticket(url).is_some()
}

/// `GET /user/me`.
#[derive(Default, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub email: Option<String>,
    /// `0` is the free tier; everything above it is a paid plan.
    #[serde(default)]
    pub plan: Option<i64>,
    /// When the paid plan runs out, as TorBox states it.
    #[serde(default)]
    pub premium_expires_at: Option<String>,
    #[serde(default)]
    pub is_subscribed: Option<bool>,
}

impl UserInfo {
    /// Whether this account is on a paid plan.
    ///
    /// The plan number decides and the subscription flag only adds to it: a person who has
    /// cancelled a subscription still has the plan until it runs out, and showing them a free
    /// account for the last month they paid for would be wrong in the direction that matters.
    #[must_use]
    pub fn is_premium(&self) -> bool {
        self.plan.is_some_and(|plan| plan > 0) || self.is_subscribed.unwrap_or(false)
    }

    /// What the accounts list shows next to the row, as something to show or nothing.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
}

/// The failure envelope, and the `data` of a `requestdl` call, which is the address itself.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
}

impl ErrorEnvelope {
    /// The error word, upper-cased, or `None` when the answer names none.
    #[must_use]
    pub fn code(&self) -> Option<String> {
        let text = self.error.as_ref()?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_ascii_uppercase())
    }
}

/// One entry of a `mylist` answer, read for what a link check needs.
#[derive(Default, Deserialize)]
pub struct JobEntry {
    #[serde(default)]
    pub download_present: Option<bool>,
    #[serde(default)]
    pub files: Vec<JobFile>,
}

/// One file inside a job.
#[derive(Default, Deserialize)]
pub struct JobFile {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub short_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl JobEntry {
    /// The file this ticket names, if the job still offers it.
    #[must_use]
    pub fn file(&self, file_id: &str) -> Option<&JobFile> {
        self.files.iter().find(|file| {
            file.id
                .as_ref()
                .and_then(|value| match value {
                    serde_json::Value::Number(number) => Some(number.to_string()),
                    serde_json::Value::String(text) => Some(text.trim().to_owned()),
                    _ => None,
                })
                .is_some_and(|id| id == file_id)
        })
    }
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

/// How long an exhausted quota is waited out.
const QUOTA_SECONDS: u64 = 3600;

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
/// The word travels as the `api_code` parameter; the `detail` sentence beside it does not,
/// because it is prose TorBox wrote and has echoed a submitted link back before now.
#[must_use]
pub fn classify_error(api_code: &str, retry_after: Option<u64>) -> ApiFailure {
    match api_code {
        "BAD_TOKEN" | "AUTH_ERROR" | "NO_AUTH" | "OAUTH_VERIFICATION_ERROR" => {
            coded(ErrorKind::AccountInvalid, messages::AUTH_INVALID, api_code)
        }
        "PLAN_RESTRICTED_FEATURE" => {
            coded(ErrorKind::Unsupported, messages::NOT_PERMITTED, api_code)
        }
        "ITEM_NOT_FOUND" | "ENDPOINT_NOT_FOUND" | "LINK_OFFLINE" => {
            coded(ErrorKind::Offline, messages::FILE_GONE, api_code)
        }
        "MONTHLY_LIMIT" | "ACTIVE_LIMIT" | "DOWNLOAD_LIMIT" | "COOLDOWN_LIMIT" => coded(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(QUOTA_SECONDS))),
            messages::LIMIT_REACHED,
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
        other => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR.0,
            message: messages::api_error(other),
            params: vec![("api_code", other.to_owned())],
        },
    }
}

/// The failure an answer describes, or `None` when it describes none.
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
        404 | 410 => Err(plain(ErrorKind::Offline, messages::FILE_GONE)),
        429 => Err(plain(
            ErrorKind::RateLimited(Some(retry_after.unwrap_or(60))),
            messages::RATE_LIMITED,
        )),
        451 => Err(plain(ErrorKind::Permanent, messages::REQUEST_REFUSED)),
        500..=599 => Err(plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_BUSY,
        )),
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

/// Reads the address a `requestdl` answer carries.
///
/// TorBox states it as the bare `data` string. Checked before it is handed on: a resolved
/// address goes straight to the transfer engine, and an answer that was not an `https` address
/// is a refusal rather than something to try fetching.
#[must_use]
pub fn read_download_address(body: &[u8]) -> Option<String> {
    let envelope: serde_json::Value = serde_json::from_slice(body).ok()?;
    let raw = envelope.get("data")?.as_str()?.trim();
    let parsed = url::Url::parse(raw).ok()?;
    matches!(parsed.scheme(), "https" | "http").then(|| parsed.to_string())
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

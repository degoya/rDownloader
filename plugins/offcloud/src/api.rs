//! Target-independent Offcloud logic: response shapes, the hoster catalogue merge and the
//! failure classification.
//!
//! Written against the provider's own API documentation, <https://offcloud.com/api> and the
//! repository it points at, <https://github.com/Offcloud/offcloud-api>:
//!
//! - **Auth flavour**: `Authorization: Bearer <api key>`. The published document also offers
//!   `?key=<api key>` as a query parameter, and the header is used instead for one reason: a
//!   query parameter ends up in every redirect chain, every proxy log and every error message
//!   that quotes an address, and a header does not. The plugin never sees the key either way —
//!   it writes `{{secret:offcloud_api_key}}` and the host substitutes the value on the way out,
//!   towards `offcloud.com` and nowhere else.
//! - **Resolve**: `POST /api/instant` with a form body carrying `url`. The answer carries the
//!   address to fetch (`url`), `fileName`, `site` and `status`. A fresh call is what renews a
//!   short-lived address: nothing here is cached, and Offcloud mints the link at the moment it
//!   is asked for.
//! - **Account**: `GET /api/account/info`, carrying `userId`, `isPremium`, `canDownload` and
//!   `expirationDate`. Both spellings of each field are accepted — the published examples and
//!   the field names JDownloader's `OffCloudCom.java` reads out of the same endpoint differ in
//!   case, and guessing wrong would silently report every account as free.
//! - **Hosters**: `GET /api/sites`, the catalogue the account's plan covers.
//! - **Refusals**: `{"error": "<sentence>"}`, and `{"not_available": "<reason>"}` when the
//!   account would need an add-on for this particular link. `NOAUTH` is the one stable word in
//!   the first shape; the reasons in the second are a documented, closed set. Everything else
//!   the provider writes is prose and is dropped rather than forwarded.
//!
//! **Nothing here has been run against a live Offcloud account.** The shapes come from the
//! provider's documentation and from two independent clients of it; `docs/roadmap/jobs/
//! 120-02-offcloud.md` records the run against a real account as open.

use serde::Deserialize;

use crate::messages;

/// The vault reference the Offcloud provider keeps its API key under. The value never reaches
/// this plugin.
pub const API_KEY_REFERENCE: &str = "offcloud_api_key";

pub const API_BASE: &str = "https://offcloud.com/api";

/// Whether a scheme is one a multihoster could be asked about at all.
#[must_use]
pub fn matches(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// `POST /api/instant`.
#[derive(Default, Deserialize)]
pub struct InstantDownload {
    /// The address to fetch. Short-lived; a new one is minted by asking again.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, alias = "fileName", alias = "filename")]
    pub file_name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// `GET /api/account/info`.
///
/// Every field carries both spellings the two documented clients use. A `serde` alias costs
/// nothing and removes the one failure mode that would not show up as an error: a camel-cased
/// answer read through snake-cased fields is a valid parse in which every account is free.
#[derive(Default, Deserialize)]
pub struct AccountInfo {
    #[serde(default, alias = "userId")]
    pub user_id: Option<String>,
    #[serde(default, alias = "isPremium")]
    pub is_premium: Option<bool>,
    #[serde(default, alias = "canDownload")]
    pub can_download: Option<bool>,
    #[serde(default, alias = "expirationDate")]
    pub expiration_date: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// One entry of `GET /api/sites`.
///
/// Offcloud describes a site by a name and the hosts it answers for, and the two documented
/// clients disagree on whether the hosts arrive as one string or as a list. Both are read, and
/// an entry that carries neither is skipped rather than guessed at.
#[derive(Default, Deserialize)]
pub struct SiteEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

/// The two refusal shapes, read out of one answer.
#[derive(Default, Deserialize)]
pub struct ErrorEnvelope {
    /// The provider's own sentence. Read so its presence can be detected, and forwarded only
    /// when it is one of the stable words below.
    #[serde(default)]
    pub error: Option<String>,
    /// Which add-on the account would need for this link, when that is what stands in the way.
    #[serde(default, alias = "notAvailable")]
    pub not_available: Option<String>,
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

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// multihoster plugins settled on.
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
/// it — an address, and with the query-parameter entrance the key itself. So the value is kept
/// only when the whole of it is a short, code-shaped token, and dropped whole otherwise:
/// filtering an answer that echoed a credential would keep its digits.
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
///
/// `NOAUTH` is the only value the provider's own clients branch on, and it is the one that
/// matters: it says the key is not a key any more, which no amount of waiting repairs.
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
        // Prose, and therefore nothing that can be shown or branched on. The category is
        // transient rather than permanent: an unreadable sentence is at least as likely to be
        // a passing outage as a verdict about this link.
        None => plain(
            ErrorKind::Transient(Some(BUSY_SECONDS)),
            messages::API_ERROR,
        ),
    }
}

/// Classifies the closed set of `not_available` reasons.
///
/// None of them is a fault of the link: each says the account's plan does not cover this kind
/// of download. `Unsupported` rather than `Permanent`, so the queue moves the link on to
/// another account or another way in instead of marking it dead.
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
        404 | 410 => Err(plain(ErrorKind::Offline, messages::LINK_GONE)),
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
/// Percent-encodes by hand rather than pulling a URL crate in for one field: the one value
/// that travels this way is an address, and a body that did not encode its `&` and `=` would
/// submit a truncated one.
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

/// The hoster catalogue, flattened to lower-case hosts and deduplicated.
///
/// One site is one row at Offcloud and several domains at the hoster it stands for, and the
/// queue matches on hosts. An entry that names no host at all contributes nothing: a
/// catalogue row is only useful here if it says which addresses it covers.
#[must_use]
pub fn merge_hosters(entries: Vec<SiteEntry>) -> Vec<String> {
    let mut hosts: Vec<String> = entries
        .into_iter()
        .flat_map(|entry| {
            entry
                .hosts
                .into_iter()
                .chain(entry.domains)
                .chain(entry.domain)
                .chain(entry.display_name.filter(|value| value.contains('.')))
                .chain(entry.name.filter(|value| value.contains('.')))
        })
        .filter_map(|host| {
            let host = host.trim().trim_start_matches("www.").to_ascii_lowercase();
            (!host.is_empty() && host.contains('.') && !host.contains('/')).then_some(host)
        })
        .collect();
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

/// Whether the account may be used for downloading at all.
///
/// Two flags rather than one, because Offcloud has two ways of saying no and they mean
/// different things: a free account has not bought the premium add-on, and an account whose
/// `canDownload` is false has bought it and is still refused — a suspension, an unpaid
/// invoice, an abuse hold. The second is reported separately so the person is not told to buy
/// something they already own.
#[must_use]
pub fn account_state(info: &AccountInfo) -> AccountState {
    match (
        info.is_premium.unwrap_or(false),
        info.can_download.unwrap_or(true),
    ) {
        (false, _) => AccountState::Free,
        (true, false) => AccountState::Blocked,
        (true, true) => AccountState::Usable,
    }
}

/// What `GET /api/account/info` says about an account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountState {
    /// Premium, and allowed to download.
    Usable,
    /// No premium add-on. Offcloud's free tier cannot be used for downloading through the API.
    Free,
    /// Premium, and refused by the provider for a reason it does not name.
    Blocked,
}

/// The name an account is shown under: the address when there is one, the opaque account
/// identifier otherwise. Never the key.
#[must_use]
pub fn account_name(info: &AccountInfo) -> Option<&str> {
    info.email
        .as_deref()
        .or(info.user_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

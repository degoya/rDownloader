//! Premiumize's protocol logic, written once for both builds.
//!
//! A multihoster with a Bearer-authenticated JSON API. Unlike the others it can answer a link
//! check without unlocking anything — `cache/check` reports, per link, whether Premiumize
//! already holds the file — so this is the one multihoster here with a real `check`.

use plugin_common::failure::{HttpError, SecretSlot, coded, require_secret};
use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, LinkCheck, LinkStatus,
    PluginHost, ResolveInput, Resolved,
};
use premiumize_common::cache::{self, CacheCheckResponse, Holding};
use premiumize_common::listing::Flexible;
use serde::Deserialize;
use url::form_urlencoded;

use crate::messages;

/// The secret every call needs, and the words its absence is refused with.
const ACCOUNT_SECRET: SecretSlot = SecretSlot {
    reference: API_KEY_REFERENCE,
    missing: messages::API_KEY_REQUIRED,
};

const API_KEY_REFERENCE: &str = "premiumize_api_key";

/// This plugin's one HTTP code: a status no document explains is classified by
/// `plugin_common::http_status`, the one mapping every plugin shares (RD-191-07), and reported
/// under it with the status as a parameter. A `429` or a `5xx` carries the response's
/// `Retry-After`.
const HTTP_ERROR: HttpError = HttpError {
    code: messages::HTTP_ERROR,
    text: messages::http_error,
};

/// Whether this plugin claims `url`. A multihoster claims by account catalogue rather than by
/// host, so anything fetchable over http(s) is a candidate.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// What the account is worth, from `account/info`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id, ACCOUNT_SECRET).await?;
    let response = request(host, "GET", "/api/account/info", Vec::new()).await?;
    let account: AccountResponse = parse_json(&response)?;
    ensure_success(
        &account.status,
        account.code.as_deref(),
        account.message.as_deref(),
    )?;
    // Compared against the host's clock. The component used to test `premium_until > 0`, which
    // reports premium for every account that ever had it, because the guest had no clock.
    let now = i64::try_from(host.now_unix_seconds().await).unwrap_or(i64::MAX);
    let premium = account
        .premium_until
        .is_some_and(|timestamp| timestamp > now);
    Ok(Account {
        valid: true,
        premium,
        label: crate::account::label(account.customer_id, account.limit_used.as_ref()).into(),
        // Premiumize exposes only the consumed fraction of the fair-use limit, never the limit
        // itself, so remaining traffic stays unknown and is reported in the label.
        traffic_left: None,
    })
}

/// Unlocks one link through `transfer/directdl`.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = input
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id, ACCOUNT_SECRET).await?;
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("src", &input.url)
        .finish()
        .into_bytes();
    let response = request(host, "POST", "/api/transfer/directdl", body).await?;
    let direct: DirectResponse = parse_json(&response)?;
    ensure_success(
        &direct.status,
        direct.code.as_deref(),
        direct.message.as_deref(),
    )?;
    // A source holding several files is no longer refused here. It used to end in
    // `premiumize.multi_file_source`, whose text told people to split the source in the
    // LinkGrabber — a step nothing implemented. The folder crawler (`plugins/premiumize-
    // crawler`, RD-104-03) is that step, and it runs before a link ever reaches a resolver.
    // What is left over is a *download* link that unlocked to several files, which resolve
    // cannot represent: it answers with one file, so it takes the first and leaves the rest
    // to the crawler that enumerated them.
    to_resolved(direct)
}

/// Turns one `transfer/directdl` answer into the single file `resolve` hands back.
///
/// Read as loosely as the answer may arrive. Premiumize documents `content[].path` as a string
/// and `content[].size` as a byte count, but the same account's `cache/check` quotes a size as
/// a string, and the deprecated top-level `location`/`filename`/`filesize` exist precisely
/// because a single-file answer once carried its file up there instead. So each of the three
/// is taken from `content` when it is there and from the top level when it is not, and only
/// the link itself — without which there is nothing to download — is required.
fn to_resolved(direct: DirectResponse) -> Result<Resolved, Failure> {
    let first = direct.content.unwrap_or_default().into_iter().next();
    let (link, path, size) = match first {
        Some(item) => (
            item.link,
            item.path.or(direct.filename),
            item.size.or(direct.filesize),
        ),
        // No usable entry: fall back to the legacy single-file fields. They are deprecated and
        // mirror the first entry, which is exactly what is wanted when there is no first entry
        // to read.
        None => (
            direct
                .location
                .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_FILE))?,
            direct.filename,
            direct.filesize,
        ),
    };
    Ok(Resolved {
        url: link,
        file_name: path.as_deref().and_then(base_name).map(str::to_owned),
        size: size.as_ref().and_then(Flexible::as_u64),
        headers: Vec::new(),
        checksum: None,
    })
}

/// The last non-empty segment of a slash-joined path inside the source.
fn base_name(path: &str) -> Option<&str> {
    path.rsplit('/').find(|part| !part.is_empty())
}

/// The hosters this account's plan covers, from `services/list`.
pub(crate) async fn hosters<H: PluginHost>(
    host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    let response = request(host, "GET", "/api/services/list", Vec::new()).await?;
    let parsed: ServicesResponse = parse_json(&response)?;
    Ok(crate::services::merge_hosters(
        parsed.cache,
        parsed.directdl,
    ))
}

/// Batched cache check: Premiumize answers with index-aligned arrays for the requested items.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    input: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let mut results = Vec::with_capacity(input.urls.len());
    for chunk in input.urls.chunks(cache::MAX_ITEMS) {
        let body = cache::check_body(chunk);
        let response = request(host, "POST", "/api/cache/check", body).await?;
        let parsed: CacheCheckResponse = parse_json(&response)?;
        ensure_success(
            &parsed.status,
            parsed.code.as_deref(),
            parsed.message.as_deref(),
        )?;
        results.extend(map_cache_check(chunk, &parsed));
    }
    Ok(results)
}

async fn request<H: PluginHost>(
    host: &H,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<HttpResponse, Failure> {
    let request = HttpRequest {
        method: method.to_owned(),
        url: format!("https://www.premiumize.me{path}"),
        query: Vec::new(),
        headers: Vec::new(),
        body,
    }
    .with_header(
        "Authorization",
        format!("Bearer {{{{secret:{API_KEY_REFERENCE}}}}}"),
    )
    .with_header("Content-Type", "application/x-www-form-urlencoded");
    plugin_common::failure::call(host, request, |status, retry_after, _| {
        HTTP_ERROR.ensure_http_status(status, retry_after).err()
    })
    .await
}

/// Maps one `cache/check` response onto the requested URLs.
///
/// `true` is `Cached`, not `Online` (RD-120-36): it says Premiumize can hand the file over
/// right now, which is more than "the file exists" and holds only until Premiumize evicts
/// it. `false` with a name is still `Online`, because Premiumize knows the file and has not
/// fetched it yet. Anything else is `Unknown`.
///
/// The reading of each position is `premiumize_common::cache`'s, shared with the transfers
/// plugin (RD-130-11); only the status vocabulary is this plugin's own.
fn map_cache_check(urls: &[String], response: &CacheCheckResponse) -> Vec<LinkCheck> {
    urls.iter()
        .zip(cache::items(urls.len(), response))
        .map(|(url, item)| LinkCheck {
            url: url.clone(),
            status: match item.holding() {
                Holding::Cached => LinkStatus::Cached,
                // Not cached but named: Premiumize knows the file, it just has not fetched it
                // yet, which is still an answer about the link being alive.
                Holding::Known => LinkStatus::Online,
                Holding::Unknown => LinkStatus::Unknown,
            },
            file_name: item.file_name,
            size: item.size,
        })
        .collect()
}

fn ensure_success(status: &str, code: Option<&str>, message: Option<&str>) -> Result<(), Failure> {
    if status == "success" {
        return Ok(());
    }
    let kind = failure_kind(code, message);
    let (api_code, default_message) = messages::API_ERROR;
    let mut failure = Failure::coded(kind, api_code, message.unwrap_or(default_message));
    if let Some(message) = message {
        failure = failure.with_param("message", message);
    }
    if let Some(code) = code {
        failure = failure.with_param("api_code", code);
    }
    Err(failure)
}

/// Premiumize's own error vocabulary, grouped the way its documentation groups it.
///
/// The `code` field is the stable identifier and the published table sorts every code into
/// transient, semi-permanent, permanent or unknown. This plugin used to match six strings —
/// `not_logged_in`, `invalid_token`, `bad_token`, `fairuse_limit`, `limit_exceeded`,
/// `unsupported_service` — that appear nowhere in that table, so every real code fell through
/// to `Permanent`: `service_down` and `rate_limit_reached` were never retried, and
/// `service_unsupported` was never reported as unsupported. The old strings are kept as
/// aliases rather than deleted, because an installation may still be answered by an older
/// deployment and nothing here can tell.
fn failure_kind(code: Option<&str>, message: Option<&str>) -> FailureKind {
    match code.unwrap_or_default() {
        "link_generation_failed" | "transient_error" | "service_down" | "unknown_error" => {
            FailureKind::Transient(None)
        }
        "service_limit_reached"
        | "account_limit_reached"
        | "rate_limit_reached"
        | "semi_permanent_error"
        | "fairuse_limit"
        | "limit_exceeded" => FailureKind::RateLimited(None),
        "authentication_failed" | "not_logged_in" | "invalid_token" | "bad_token" => {
            FailureKind::AccountInvalid
        }
        "service_unsupported" | "unsupported" | "unsupported_service" => FailureKind::Unsupported,
        "not_found" | "permission_denied" | "invalid_request" | "permanent_error" => {
            FailureKind::Permanent
        }
        // No code, or one this table has never seen. The envelope carries nothing else but the
        // message, so read that rather than calling every unknown answer permanent: the link a
        // multihoster will not take is the one case where the difference changes what the
        // interface may offer.
        _ => kind_from_message(message),
    }
}

/// Last resort when no code was sent: premiumize's messages are English and fixed phrases.
fn kind_from_message(message: Option<&str>) -> FailureKind {
    let Some(message) = message else {
        return FailureKind::Permanent;
    };
    let message = message.to_ascii_lowercase();
    if message.contains("unsupported") || message.contains("not supported") {
        FailureKind::Unsupported
    } else {
        FailureKind::Permanent
    }
}

/// Reads one answer, and says which field it could not read.
///
/// A body that does not fit is **permanent**. It used to be `Transient`, which the scheduler
/// retries: an answer whose shape is wrong is wrong again on every attempt, so the job walked
/// to `max_retries` and could never succeed. The serde error used to be discarded along with
/// it; `serde_path_to_error` keeps the field path — `content[0].size` — which is the whole of
/// what a report needs and none of the values, which may carry a signed link.
fn parse_json<T: for<'de> Deserialize<'de>>(response: &HttpResponse) -> Result<T, Failure> {
    let mut deserializer = serde_json::Deserializer::from_slice(&response.body);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let field = error.path().to_string();
        // An empty path, or serde's root marker, means nothing inside was reached: the body is
        // not this API's envelope at all, and naming a field would be a guess.
        if field.is_empty() || field == "." {
            return coded(FailureKind::Permanent, messages::INVALID_RESPONSE);
        }
        let (code, _) = messages::INVALID_RESPONSE_FIELD;
        Failure::coded(
            FailureKind::Permanent,
            code,
            messages::invalid_response_field(&field),
        )
        .with_param("field", field)
    })
}

#[derive(Deserialize)]
struct AccountResponse {
    status: String,
    customer_id: Option<String>,
    premium_until: Option<i64>,
    limit_used: Option<crate::account::FairUse>,
    code: Option<String>,
    message: Option<String>,
}

/// `POST /api/transfer/directdl`: the unlocked file or files, plus the legacy single-file
/// fields premiumize still sends beside them.
#[derive(Deserialize)]
struct DirectResponse {
    status: String,
    /// Absent and `null` mean the same thing here: no entry to read.
    #[serde(default)]
    content: Option<Vec<DirectFile>>,
    /// Deprecated top-level mirrors of the first `content` entry's `link`, `path` and `size`.
    /// Read only as a fallback, and only because an answer that carries no `content` carries
    /// nothing else.
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    filesize: Option<Flexible>,
    code: Option<String>,
    message: Option<String>,
}

/// One entry of `content`. Only `link` is required: without it there is nothing to fetch,
/// while a missing name or size is an answer this plugin can still work with.
#[derive(Deserialize)]
struct DirectFile {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    size: Option<Flexible>,
    link: String,
}

/// `GET /api/services/list`: hoster domains grouped by capability.
#[derive(Deserialize)]
struct ServicesResponse {
    #[serde(default)]
    cache: Vec<String>,
    #[serde(default)]
    directdl: Vec<String>,
}

#[cfg(test)]
#[path = "resolver/tests.rs"]
mod tests;

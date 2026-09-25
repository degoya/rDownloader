//! Premiumize's protocol logic, written once for both builds.
//!
//! A multihoster with a Bearer-authenticated JSON API. Unlike the others it can answer a link
//! check without unlocking anything — `cache/check` reports, per link, whether Premiumize
//! already holds the file — so this is the one multihoster here with a real `check`.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, LinkCheck, LinkStatus,
    PluginHost, ResolveInput, Resolved,
};
use premiumize_common::cache::{self, CacheCheckResponse, Holding};
use serde::Deserialize;
use url::form_urlencoded;

use crate::messages;

const API_KEY_REFERENCE: &str = "premiumize_api_key";

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
    require_secret(host, account_id).await?;
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
    require_secret(host, account_id).await?;
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
        size: size.as_ref().and_then(FlexibleU64::as_u64),
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
    let response = host
        .http(
            HttpRequest {
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
            .with_header("Content-Type", "application/x-www-form-urlencoded"),
        )
        .await?;
    ensure_http_status(&response)?;
    Ok(response)
}

async fn require_secret<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host.secret_available(account_id, API_KEY_REFERENCE).await {
        return Err(coded(FailureKind::AuthRequired, messages::API_KEY_REQUIRED));
    }
    Ok(())
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

fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    let kind = match response.status {
        200..=299 => return Ok(()),
        401 | 403 => FailureKind::AccountInvalid,
        429 => FailureKind::RateLimited(None),
        500..=599 => FailureKind::Transient(None),
        _ => FailureKind::Permanent,
    };
    Err(Failure::coded(
        kind,
        messages::HTTP_ERROR,
        messages::http_error(response.status),
    )
    .with_param("status", response.status.to_string()))
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

fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
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
    filesize: Option<FlexibleU64>,
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
    size: Option<FlexibleU64>,
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

#[derive(Deserialize)]
#[serde(untagged)]
enum FlexibleU64 {
    Number(u64),
    Float(f64),
    Text(String),
}

impl FlexibleU64 {
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Float(value) if *value >= 0.0 => Some(*value as u64),
            Self::Float(_) => None,
            Self::Text(value) => value.parse().ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `transfer/directdl` bodies below.
    ///
    /// **Constructed, not captured.** They are written from premiumize's published API
    /// documentation (<https://www.premiumize.me/api>, read 2026-09-20) plus the shapes that
    /// page names as still being sent: the deprecated top-level `location`/`filename`/
    /// `filesize` mirrors of the first `content` entry, the deprecated `content[].stream_link`
    /// which is `null` for a non-video, and `content[].transcode_status`. The documentation
    /// shows `size` as an integer for this endpoint and as a quoted string for
    /// `cache/check`; nobody here has a live account, so the quoted and `null` variants are
    /// what this plugin *tolerates*, not what anybody measured. Every URL is
    /// `download.example.test`; no fixture carries an API key, a token or a real CDN link.
    const DOCUMENTED: &str = include_str!("../tests/fixtures/directdl-content-documented.json");
    const QUOTED_SIZE: &str = include_str!("../tests/fixtures/directdl-content-quoted-size.json");
    const NULL_PATH: &str = include_str!("../tests/fixtures/directdl-content-null-path.json");
    const NULL_CONTENT: &str = include_str!("../tests/fixtures/directdl-content-null.json");
    const LEGACY_SINGLE: &str = include_str!("../tests/fixtures/directdl-legacy-single-file.json");
    const UNSUPPORTED: &str =
        include_str!("../tests/fixtures/directdl-error-service-unsupported.json");
    const SERVICE_DOWN: &str = include_str!("../tests/fixtures/directdl-error-service-down.json");
    const NO_CODE: &str = include_str!("../tests/fixtures/directdl-error-without-code.json");

    /// Reads a fixture the way `resolve` reads a response body.
    fn direct(payload: &str) -> Result<Resolved, Failure> {
        let response = HttpResponse {
            status: 200,
            final_url: "https://www.premiumize.me/api/transfer/directdl".to_owned(),
            headers: Vec::new(),
            body: payload.as_bytes().to_vec(),
        };
        let parsed: DirectResponse = parse_json(&response)?;
        ensure_success(
            &parsed.status,
            parsed.code.as_deref(),
            parsed.message.as_deref(),
        )?;
        to_resolved(parsed)
    }

    #[test]
    fn direct_download_reads_the_documented_shape() {
        let resolved = direct(DOCUMENTED).expect("documented shape");
        assert_eq!(
            resolved.url, "https://download.example.test/video1.mkv",
            "the first content entry is the file resolve hands back"
        );
        assert_eq!(resolved.file_name.as_deref(), Some("video1.mkv"));
        assert_eq!(resolved.size, Some(123_456_789));
    }

    #[test]
    fn direct_download_accepts_a_size_quoted_as_a_string() {
        let resolved = direct(QUOTED_SIZE).expect("a quoted size is still a size");
        assert_eq!(resolved.size, Some(425_146_614));
        assert_eq!(
            resolved.file_name.as_deref(),
            Some("outlander.s08e01.german.bdrip.x264-intention.rar")
        );
    }

    #[test]
    fn direct_download_accepts_a_null_path_and_falls_back_to_the_top_level_name() {
        let resolved = direct(NULL_PATH).expect("a nameless entry is still a link");
        assert_eq!(
            resolved.url,
            "https://download.example.test/8x6wertoi51r8vptrojn"
        );
        assert_eq!(
            resolved.file_name.as_deref(),
            Some("outlander.s08e01.german.bdrip.x264-intention.rar"),
            "the deprecated top-level filename is the only name left"
        );
        assert_eq!(resolved.size, Some(425_146_614));
    }

    #[test]
    fn direct_download_treats_a_null_content_like_a_missing_one() {
        let resolved = direct(NULL_CONTENT).expect("null content falls back to the top level");
        assert_eq!(
            resolved.url,
            "https://download.example.test/8x6wertoi51r8vptrojn"
        );
        assert_eq!(
            resolved.file_name.as_deref(),
            Some("outlander.s08e01.german.bdrip.x264-intention.rar")
        );
        assert_eq!(resolved.size, Some(425_146_614));
    }

    #[test]
    fn direct_download_reads_the_legacy_single_file_answer() {
        let resolved = direct(LEGACY_SINGLE).expect("legacy single-file answer");
        assert_eq!(
            resolved.url,
            "https://download.example.test/8x6wertoi51r8vptrojn"
        );
        assert_eq!(
            resolved.file_name.as_deref(),
            Some("outlander.s08e01.german.bdrip.x264-intention.rar")
        );
        assert_eq!(resolved.size, Some(425_146_614));
    }

    #[test]
    fn an_answer_with_no_file_at_all_is_permanent() {
        let failure = direct(r#"{"status":"success","content":[]}"#).expect_err("no file");
        assert_eq!(failure.kind, FailureKind::Permanent);
        assert_eq!(failure.code.as_deref(), Some("premiumize.no_file"));
    }

    #[test]
    fn a_field_this_plugin_cannot_read_is_permanent_and_names_itself() {
        let failure = direct(r#"{"status":"success","content":[{"link":true}]}"#)
            .expect_err("a boolean is not a link");
        assert_eq!(
            failure.kind,
            FailureKind::Permanent,
            "a body that will never parse must not be retried to max_retries"
        );
        assert_eq!(
            failure.code.as_deref(),
            Some("premiumize.invalid_response_field")
        );
        assert_eq!(
            failure
                .params
                .iter()
                .find(|(name, _)| name == "field")
                .map(|(_, value)| value.as_str()),
            Some("content[0].link"),
            "the reason must name the field instead of discarding it"
        );
    }

    #[test]
    fn a_body_that_is_not_this_api_is_permanent() {
        let failure = direct("<html>maintenance</html>").expect_err("not JSON");
        assert_eq!(failure.kind, FailureKind::Permanent);
        assert_eq!(failure.code.as_deref(), Some("premiumize.invalid_response"));
    }

    #[test]
    fn the_documented_unsupported_code_is_read_as_unsupported() {
        let failure = direct(UNSUPPORTED).expect_err("unsupported service");
        assert_eq!(
            failure.kind,
            FailureKind::Unsupported,
            "`service_unsupported` is premiumize's documented code for this"
        );
        assert_eq!(failure.code.as_deref(), Some("premiumize.api_error"));
    }

    #[test]
    fn a_service_that_is_down_stays_retryable() {
        let failure = direct(SERVICE_DOWN).expect_err("service down");
        assert_eq!(
            failure.kind,
            FailureKind::Transient(None),
            "a documented transient code must not be buried in the permanent arm"
        );
    }

    #[test]
    fn an_error_without_a_code_is_judged_by_its_message() {
        let failure = direct(NO_CODE).expect_err("no code");
        assert_eq!(failure.kind, FailureKind::Unsupported);
        assert_eq!(
            failure
                .params
                .iter()
                .find(|(name, _)| name == "message")
                .map(|(_, value)| value.as_str()),
            Some("Unsupported link for direct download.")
        );
    }

    #[test]
    fn an_error_that_says_nothing_useful_stays_permanent() {
        let failure = direct(r#"{"status":"error","message":"Something went wrong."}"#)
            .expect_err("unknown error");
        assert_eq!(failure.kind, FailureKind::Permanent);
    }

    /// The label as `code(name=value)` parts, which is what the interface translates.
    fn account_label(payload: &str) -> String {
        let account: AccountResponse = serde_json::from_str(payload).expect("parse");
        assert_eq!(account.status, "success");
        crate::account::label(account.customer_id, account.limit_used.as_ref())
            .into_parts()
            .into_iter()
            .map(|part| {
                let params: Vec<String> = part
                    .params
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                format!("{}({})", part.code, params.join(","))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn account_info_reports_the_fair_use_share() {
        assert_eq!(
            account_label(
                r#"{"status":"success","customer_id":"4711","limit_used":0.4235,"space_used":1234567,"premium_until":1893456000}"#
            ),
            "plugin.account.user(user=4711) premiumize.account.fair_use(percent=42)"
        );
    }

    #[test]
    fn account_info_without_optional_fields_stays_valid() {
        assert_eq!(account_label(r#"{"status":"success"}"#), "");
        assert_eq!(
            account_label(r#"{"status":"success","customer_id":"4711"}"#),
            "plugin.account.user(user=4711)"
        );
    }

    #[test]
    fn account_info_tolerates_unexpected_limit_shapes() {
        // A textual fraction still counts, anything else is dropped instead of failing.
        assert_eq!(
            account_label(r#"{"status":"success","customer_id":"4711","limit_used":"0.5"}"#),
            "plugin.account.user(user=4711) premiumize.account.fair_use(percent=50)"
        );
        for limit in ["null", "true", r#"{"used":0.5}"#, r#""many""#, "-0.2"] {
            assert_eq!(
                account_label(&format!(
                    r#"{{"status":"success","customer_id":"4711","limit_used":{limit}}}"#
                )),
                "plugin.account.user(user=4711)"
            );
        }
    }

    #[test]
    fn account_info_caps_an_exceeded_limit() {
        assert_eq!(
            account_label(r#"{"status":"success","customer_id":"4711","limit_used":1.4}"#),
            "plugin.account.user(user=4711) premiumize.account.fair_use(percent=100)"
        );
    }

    #[test]
    fn cache_check_maps_index_aligned_arrays() {
        let response: CacheCheckResponse = serde_json::from_str(
            r#"{"status":"success","response":[true,false],"filename":["a.rar",null],"filesize":["1024",null]}"#,
        )
        .expect("parse");
        let urls: Vec<String> = ["https://h/a", "https://h/b"]
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
        let results = map_cache_check(&urls, &response);
        assert_eq!(results[0].status, LinkStatus::Cached);
        assert_eq!(results[0].file_name.as_deref(), Some("a.rar"));
        assert_eq!(results[0].size, Some(1024));
        assert_eq!(results[1].status, LinkStatus::Unknown);
    }

    /// RD-120-36: a cached file is `Cached`, and a file Premiumize knows but has not fetched
    /// is `Online`. The two no longer collapse into one answer.
    #[test]
    fn cache_check_keeps_cached_apart_from_known() {
        let response: CacheCheckResponse = serde_json::from_str(
            r#"{"status":"success","response":[true,false,false],"filename":["a.rar","b.rar",null],"filesize":["1024","2048",null]}"#,
        )
        .expect("parse");
        let urls: Vec<String> = ["https://h/a", "https://h/b", "https://h/c"]
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
        let statuses: Vec<LinkStatus> = map_cache_check(&urls, &response)
            .into_iter()
            .map(|result| result.status)
            .collect();
        assert_eq!(
            statuses,
            [LinkStatus::Cached, LinkStatus::Online, LinkStatus::Unknown]
        );
    }
}

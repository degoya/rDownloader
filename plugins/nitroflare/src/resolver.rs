//! Nitroflare's protocol logic, written once for both builds.
//!
//! Two entirely different paths behind one resolver: an account resolves through the documented
//! API, while an account-less link goes through the website's own free flow — start the
//! server-side countdown, solve its captcha, wait, ask for the link. `crate::api` and
//! `crate::page` already held the parts that touch no host; what moves here is everything that
//! talks to one.

mod free;

use std::collections::HashMap;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};
use serde::Deserialize;
use url::Url;

use crate::{api, messages};

/// JD batches at most 100 file ids per `getFileInfo` call (`NitroFlareCom#checkLinks`).
const FILE_INFO_BATCH_SIZE: usize = 100;

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(api::file_id)
        .is_some()
}

/// The hoster this account can download from.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(vec![api::MATCH_HOST.to_owned()])
}

/// What the premium key is worth, from `getKeyInfo`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let response = call(host, "getKeyInfo", authenticated_query()).await?;
    let envelope: api::Envelope<api::KeyInfoResult> = parse_json(&response)?;
    if let Some(failure) = api::error_from_envelope(envelope.code, envelope.message.as_deref()) {
        return Err(convert_failure(failure));
    }
    let result = envelope.result.ok_or_else(invalid_response)?;
    let active = result
        .status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case("active"));
    let expiry = result.expiry_date.as_ref().and_then(api::expiry_text);
    let premium = active && expiry.is_some();
    Ok(Account {
        valid: true,
        premium,
        label: Label::new()
            .premium_until(expiry.as_deref().filter(|_| premium))
            .into(),
        traffic_left: result.traffic_left.and_then(api::FlexibleU64::into_u64),
    })
}

/// Resolves through the API with an account, through the website's free flow without one.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url).map_err(|error| invalid_url(&error))?;
    let file_id = api::file_id(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?
        .to_owned();
    let Some(account_id) = request.account_id.as_deref() else {
        return free::resolve(host, &file_id).await;
    };
    require_secret(host, account_id).await?;
    let metadata = file_info_one(host, &file_id).await?;
    let raw_url = download_link(host, &file_id).await?;
    let url = api::parse_download_url(&raw_url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: metadata.name,
        size: metadata.size.and_then(api::FlexibleU64::into_u64),
        headers: Vec::new(),
        checksum: None,
    })
}

/// Batched availability check; `getFileInfo` needs no credential.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let coded: Vec<(String, Option<String>)> = request
        .urls
        .iter()
        .map(|url| {
            let id = Url::parse(url)
                .ok()
                .as_ref()
                .and_then(api::file_id)
                .map(str::to_owned);
            (url.clone(), id)
        })
        .collect();
    let mut results = Vec::with_capacity(coded.len());
    for chunk in coded.chunks(FILE_INFO_BATCH_SIZE) {
        let ids: Vec<&str> = chunk.iter().filter_map(|(_, id)| id.as_deref()).collect();
        let batch = if ids.is_empty() {
            Ok(HashMap::new())
        } else {
            file_info_batch(host, &ids).await
        };
        for (url, id) in chunk {
            results.push(check_result(url, id.as_deref(), &batch));
        }
    }
    Ok(results)
}

fn check_result(
    url: &str,
    id: Option<&str>,
    batch: &Result<HashMap<String, api::FileEntry>, Failure>,
) -> LinkCheck {
    let (Some(id), Ok(files)) = (id, batch) else {
        return LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Unknown,
            file_name: None,
            size: None,
        };
    };
    match files.get(id) {
        Some(entry) if entry.is_online() => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Online,
            file_name: entry.name.clone(),
            size: entry.size.clone().and_then(api::FlexibleU64::into_u64),
        },
        // A missing entry and a present-but-not-online status both mean unavailable, exactly as
        // JD treats them.
        _ => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Offline,
            file_name: None,
            size: None,
        },
    }
}

/// `GET /getDownloadLink?user=…&premiumKey=…&file=<id>` — authenticated, one file id.
async fn download_link<H: PluginHost>(host: &H, file_id: &str) -> Result<String, Failure> {
    let mut query = authenticated_query();
    query.push(Header::new("file", file_id));
    let response = call(host, "getDownloadLink", query).await?;
    let envelope: api::Envelope<api::DownloadLinkResult> = parse_json(&response)?;
    if let Some(failure) = api::error_from_envelope(envelope.code, envelope.message.as_deref()) {
        return Err(convert_failure(failure));
    }
    envelope
        .result
        .ok_or_else(invalid_response)?
        .url
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))
}

/// `GET /getFileInfo?files=<id1,id2,…>` — unauthenticated, entries keyed by file id.
async fn file_info_batch<H: PluginHost>(
    host: &H,
    ids: &[&str],
) -> Result<HashMap<String, api::FileEntry>, Failure> {
    let response = call(
        host,
        "getFileInfo",
        vec![Header::new("files", ids.join(","))],
    )
    .await?;
    let envelope: api::Envelope<api::FileInfoResult> = parse_json(&response)?;
    if let Some(failure) = api::error_from_envelope(envelope.code, envelope.message.as_deref()) {
        return Err(convert_failure(failure));
    }
    Ok(envelope
        .result
        .map(|result| result.files)
        .unwrap_or_default())
}

/// Availability and metadata for one file. A missing entry and a non-`"online"` status both
/// report `nitroflare.file_offline`, the way JD treats them.
async fn file_info_one<H: PluginHost>(host: &H, file_id: &str) -> Result<api::FileEntry, Failure> {
    let mut files = file_info_batch(host, &[file_id]).await?;
    let entry = files
        .remove(file_id)
        .ok_or_else(|| coded(FailureKind::Offline, messages::FILE_OFFLINE))?;
    if !entry.is_online() {
        return Err(coded(FailureKind::Offline, messages::FILE_OFFLINE));
    }
    Ok(entry)
}

/// `user={{username}}&premiumKey={{secret:…}}`, the query prefix every authenticated endpoint
/// requires. Neither value enters the plugin.
fn authenticated_query() -> Vec<Header> {
    vec![
        Header::new("user", "{{username}}"),
        Header::new(
            "premiumKey",
            format!("{{{{secret:{}}}}}", crate::PREMIUM_KEY_REFERENCE),
        ),
    ]
}

async fn call<H: PluginHost>(
    host: &H,
    path: &str,
    query: Vec<Header>,
) -> Result<HttpResponse, Failure> {
    let response = host
        .http(HttpRequest {
            method: "GET".to_owned(),
            url: format!("{}/{path}", api::API_BASE),
            query,
            headers: Vec::new(),
            body: Vec::new(),
        })
        .await?;
    ensure_http_status(&response)?;
    Ok(response)
}

/// Fails before any request when the account has no premium key. `getFileInfo` is
/// unauthenticated and deliberately does not call this.
async fn require_secret<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, crate::PREMIUM_KEY_REFERENCE)
        .await
    {
        return Err(coded(
            FailureKind::AuthRequired,
            messages::PREMIUM_KEY_MISSING,
        ));
    }
    Ok(())
}

pub(crate) fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    api::ensure_http_status(response.status).map_err(convert_failure)
}

pub(crate) fn convert_failure(failure: api::ApiFailure) -> Failure {
    let kind = match failure.kind {
        api::ErrorKind::Transient(seconds) => FailureKind::Transient(seconds),
        api::ErrorKind::Permanent => FailureKind::Permanent,
        api::ErrorKind::Offline => FailureKind::Offline,
        api::ErrorKind::AuthRequired => FailureKind::AuthRequired,
        api::ErrorKind::AccountInvalid => FailureKind::AccountInvalid,
        api::ErrorKind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
        api::ErrorKind::NeedsCaptcha => FailureKind::NeedsCaptcha,
        api::ErrorKind::Unsupported => FailureKind::Unsupported,
    };
    let mut built = Failure::coded(kind, failure.code, failure.message);
    for (name, value) in failure.params {
        built = built.with_param(name, value);
    }
    built
}

fn parse_json<T: for<'de> Deserialize<'de>>(response: &HttpResponse) -> Result<T, Failure> {
    serde_json::from_slice(&response.body).map_err(|_| invalid_response())
}

fn invalid_response() -> Failure {
    coded(FailureKind::Transient(None), messages::INVALID_RESPONSE)
}

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        messages::INVALID_URL,
        messages::invalid_url(error),
    )
    .with_param("error", error.to_string())
}

pub(crate) fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

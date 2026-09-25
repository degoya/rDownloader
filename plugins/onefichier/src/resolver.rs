//! 1fichier's protocol logic, written once for both builds.
//!
//! Two paths behind one resolver: an API key resolves through the documented JSON API, an
//! account-less link through the website's own page/form flow. `crate::api` and `crate::page`
//! already held the parts that touch no host; what moves here is everything that talks to one.

mod free;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LabelPart,
    LinkCheck, LinkStatus, PluginHost, ResolveInput, Resolved,
};
use serde::Deserialize;
use url::Url;

use crate::{api, messages};

const API_BASE: &str = "https://api.1fichier.com/v1";

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(api::file_id)
        .is_some()
}

/// The hoster domains this plugin serves.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(api::MATCH_HOSTS
        .iter()
        .map(|host| (*host).to_owned())
        .collect())
}

/// What the account is worth, from `user/info.cgi`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let response = call(host, "/user/info.cgi", b"{}".to_vec()).await?;
    let info: api::UserInfoResponse = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(info.status.as_deref(), info.message.as_deref()) {
        return Err(convert_failure(failure));
    }
    let offer = info.offer.and_then(api::FlexibleU64::into_u64);
    let premium = offer.is_some_and(|value| value >= 1);
    Ok(Account {
        valid: true,
        premium,
        label: Label::new()
            .user(info.email.as_deref())
            // Offer `2` is the "Access" plan only 1fichier has, so its name is this plugin's
            // to translate; `0` (free) and `1` (premium) are said by the flag next to it.
            .maybe(
                (offer == Some(2))
                    .then(|| LabelPart::coded(messages::PLAN_ACCESS.0, messages::PLAN_ACCESS.1)),
            )
            .premium_until(info.subscription_end.as_deref().filter(|_| premium))
            .into(),
        traffic_left: info
            .cdn
            .and_then(api::FlexibleU64::into_u64)
            .and_then(|gigabytes| gigabytes.checked_mul(1024 * 1024 * 1024)),
    })
}

/// Resolves through the API with a key, through the website flow without one.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url).map_err(|error| invalid_url(&error))?;
    let Some(account_id) = request.account_id.as_deref() else {
        return free::resolve(host, &parsed).await;
    };
    require_secret(host, account_id).await?;
    let link = api::canonical_link(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?;
    let metadata = file_info(host, &link).await?;
    let response = call(host, "/download/get_token.cgi", api::link_body(&link)).await?;
    let token: api::GetTokenResponse = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(token.status.as_deref(), token.message.as_deref())
    {
        return Err(convert_failure(failure));
    }
    let url = token
        .url
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: metadata.filename,
        size: metadata.size.and_then(api::FlexibleU64::into_u64),
        headers: Vec::new(),
        // 1fichier only exposes a Whirlpool checksum, which the contract does not carry.
        checksum: None,
    })
}

/// One link at a time: `file/info.cgi` takes no batch, so one link's problem must not
/// invalidate the others.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id).await?;
    let mut results = Vec::with_capacity(request.urls.len());
    for url in &request.urls {
        results.push(check_one(host, url).await);
    }
    Ok(results)
}

/// One `check()` entry. Anything but a definitive online/offline answer degrades to `Unknown`
/// rather than failing the whole batch.
async fn check_one<H: PluginHost>(host: &H, url: &str) -> LinkCheck {
    let link = Url::parse(url).ok().as_ref().and_then(api::canonical_link);
    let Some(link) = link else {
        return unknown(url);
    };
    match file_info_status(host, &link).await {
        Ok(Some(info)) => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Online,
            file_name: info.filename,
            size: info.size.and_then(api::FlexibleU64::into_u64),
        },
        Ok(None) => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Offline,
            file_name: None,
            size: None,
        },
        Err(_) => unknown(url),
    }
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}

/// `file/info.cgi` for one link. `Ok(Some(_))` is online with metadata, `Ok(None)` a confirmed
/// offline file, `Err` anything else.
async fn file_info_status<H: PluginHost>(
    host: &H,
    link: &str,
) -> Result<Option<api::FileInfoResponse>, Failure> {
    let response = raw_call(host, "/file/info.cgi", api::link_body(link)).await?;
    if response.status == 404 {
        return Ok(None);
    }
    ensure_http_status(&response)?;
    let info: api::FileInfoResponse = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(info.status.as_deref(), info.message.as_deref()) {
        return if matches!(failure.kind, api::ErrorKind::Offline) {
            Ok(None)
        } else {
            Err(convert_failure(failure))
        };
    }
    Ok(Some(info))
}

/// As [`file_info_status`], but an offline file fails: `resolve` cannot hand one back.
async fn file_info<H: PluginHost>(host: &H, link: &str) -> Result<api::FileInfoResponse, Failure> {
    file_info_status(host, link)
        .await?
        .ok_or_else(|| coded(FailureKind::Offline, messages::FILE_OFFLINE))
}

async fn call<H: PluginHost>(host: &H, path: &str, body: Vec<u8>) -> Result<HttpResponse, Failure> {
    let response = raw_call(host, path, body).await?;
    ensure_http_status(&response)?;
    Ok(response)
}

async fn raw_call<H: PluginHost>(
    host: &H,
    path: &str,
    body: Vec<u8>,
) -> Result<HttpResponse, Failure> {
    host.http(
        HttpRequest {
            method: "POST".to_owned(),
            url: format!("{API_BASE}{path}"),
            query: Vec::new(),
            headers: Vec::new(),
            body,
        }
        .with_header(
            "Authorization",
            format!("Bearer {{{{secret:{}}}}}", crate::API_KEY_REFERENCE),
        )
        .with_header("Content-Type", "application/json"),
    )
    .await
}

async fn require_secret<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, crate::API_KEY_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::API_KEY_MISSING));
    }
    Ok(())
}

fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
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
    serde_json::from_slice(&response.body)
        .map_err(|_| coded(FailureKind::Transient(None), messages::INVALID_RESPONSE))
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

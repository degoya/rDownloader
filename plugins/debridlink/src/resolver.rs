//! Debrid-Link's protocol logic, written once for both builds.
//!
//! A multihoster with a Bearer-authenticated JSON API and nothing else. `crate::api` already
//! held everything that touches no host — the envelope shape, the error classification, the
//! hoster catalogue merge — so what moves here is the sequence of calls that used to exist once
//! per build.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LinkCheck,
    PluginHost, ResolveInput, Resolved,
};
use serde::Deserialize;

use crate::{api, messages};

/// Whether this plugin claims `url`. A multihoster claims by account catalogue rather than by
/// host, so anything fetchable over http(s) is a candidate.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| api::matches(url.scheme()))
}

/// What the account is worth, from `/account/infos`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let response = call(host, "GET", "/account/infos", Vec::new()).await?;
    let envelope: api::Envelope<api::AccountInfoData> = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(envelope.success, envelope.error.as_deref()) {
        return Err(convert_failure(failure));
    }
    let data = envelope.value.unwrap_or_default();
    Ok(Account {
        valid: true,
        premium: api::is_premium(data.account_type),
        label: Label::new()
            .user(api::account_name(
                data.pseudo.as_deref(),
                data.email.as_deref(),
            ))
            .into(),
        traffic_left: None,
    })
}

/// Hands the link to `/downloader/add` and takes the direct URL it answers with.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id).await?;
    let response = call(host, "POST", "/downloader/add", api::add_body(&request.url)).await?;
    let envelope: api::Envelope<api::AddData> = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(envelope.success, envelope.error.as_deref()) {
        return Err(convert_failure(failure));
    }
    let data = envelope.value.unwrap_or_default();
    let raw_url = data
        .download_url
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&raw_url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: data.name,
        size: data.size,
        headers: Vec::new(),
        checksum: None,
    })
}

/// The hosters this account's plan covers, as the provider reports them.
pub(crate) async fn hosters<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Vec<String>, Failure> {
    require_secret(host, account_id).await?;
    let response = call(
        host,
        "GET",
        "/downloader/hosts?keys=status,isFree,name,domains",
        Vec::new(),
    )
    .await?;
    let envelope: api::Envelope<Vec<api::HostEntry>> = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(envelope.success, envelope.error.as_deref()) {
        return Err(convert_failure(failure));
    }
    Ok(api::merge_hosters(envelope.value.unwrap_or_default()))
}

/// Debrid-Link has no link-check endpoint; adding the link is the only way to learn anything.
pub(crate) async fn check<H: PluginHost>(
    _host: &H,
    _request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    Err(coded(FailureKind::Unsupported, messages::CHECK_UNSUPPORTED))
}

/// Calls `{API_BASE}{path}` with the Bearer-authenticated headers and fails on any non-2xx.
///
/// The key never enters the plugin: `{{secret:…}}` is expanded by the host.
async fn call<H: PluginHost>(
    host: &H,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<HttpResponse, Failure> {
    let request = HttpRequest {
        method: method.to_owned(),
        url: format!("{}{path}", api::API_BASE),
        query: Vec::new(),
        headers: Vec::new(),
        body,
    }
    .with_header(
        "Authorization",
        format!("Bearer {{{{secret:{}}}}}", api::API_KEY_REFERENCE),
    )
    .with_header("Content-Type", "application/x-www-form-urlencoded");
    let response = host.http(request).await?;
    api::ensure_http_status(response.status).map_err(convert_failure)?;
    Ok(response)
}

/// Fails before any request when the account has no API key: every endpoint requires one.
async fn require_secret<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, api::API_KEY_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::API_KEY_MISSING));
    }
    Ok(())
}

fn convert_failure(failure: api::ApiFailure) -> Failure {
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

fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

//! Rapidgator's protocol logic, written once for both builds.
//!
//! Session-based: every invocation logs in for a fresh token, because a WebAssembly guest is
//! stateless and there is nowhere to cache one. The account-less path is the website's own
//! countdown-and-captcha flow instead.

mod free;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, Label, LinkCheck, LinkStatus,
    PluginHost, ResolveInput, Resolved,
};
use serde::de::DeserializeOwned;
use url::Url;

use crate::{api, messages};

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

/// What the account is worth: the login answer carries it.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let user = login(host).await?.user;
    let premium = user
        .as_ref()
        .and_then(|user| user.is_premium)
        .unwrap_or(false);
    Ok(Account {
        valid: true,
        premium,
        label: Label::new()
            .premium_until(
                api::premium_until(
                    premium,
                    user.as_ref().and_then(|user| user.premium_end_time),
                )
                .as_deref(),
            )
            .into(),
        traffic_left: user
            .as_ref()
            .and_then(|user| user.traffic.as_ref())
            .and_then(|traffic| traffic.left)
            .and_then(|value| u64::try_from(value).ok()),
    })
}

/// Resolves through the API with an account, through the website flow without one.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url).map_err(|error| invalid_url(&error))?;
    let file_id = api::file_id(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?
        .to_owned();
    let Some(account_id) = request.account_id.as_deref() else {
        return free::resolve(host, &parsed, &file_id).await;
    };
    require_secret(host, account_id).await?;
    let token = login(host)
        .await?
        .token
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_TOKEN))?;
    let metadata = file_info(host, &token, &file_id).await?;
    let raw_url = download_url(host, &token, &file_id).await?;
    let url = api::parse_download_url(&raw_url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: metadata.name,
        size: metadata.size.and_then(|size| u64::try_from(size).ok()),
        headers: Vec::new(),
        checksum: None,
    })
}

/// One link at a time: `file/info` takes no batch, so one link's problem must not invalidate
/// the others.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id).await?;
    let Some(token) = login(host).await?.token else {
        return Ok(request.urls.iter().map(|url| unknown(url)).collect());
    };
    let mut results = Vec::with_capacity(request.urls.len());
    for url in &request.urls {
        results.push(check_one(host, &token, url).await);
    }
    Ok(results)
}

/// One `check()` entry. Anything but a definitive online/offline answer degrades to `Unknown`.
async fn check_one<H: PluginHost>(host: &H, token: &str, url: &str) -> LinkCheck {
    let file_id = Url::parse(url)
        .ok()
        .as_ref()
        .and_then(api::file_id)
        .map(str::to_owned);
    let Some(file_id) = file_id else {
        return unknown(url);
    };
    match file_info_status(host, token, &file_id).await {
        Ok(Some(entry)) => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Online,
            file_name: entry.name,
            size: entry.size.and_then(|size| u64::try_from(size).ok()),
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

/// `GET user/login?login={{username}}&password={{secret:…}}` — a fresh session token every
/// invocation; never cached, because there is nowhere to cache it. A 404 here is not a
/// meaningful signal either way, so it stays trusted.
async fn login<H: PluginHost>(host: &H) -> Result<api::LoginResult, Failure> {
    api_call(
        host,
        "user/login",
        vec![
            Header::new("login", "{{username}}"),
            Header::new(
                "password",
                format!("{{{{secret:{}}}}}", crate::PASSWORD_REFERENCE),
            ),
        ],
        true,
    )
    .await
}

/// `token=<t>&file_id=<id>`, the query `file/info` and `file/download` share.
fn file_query(token: &str, file_id: &str) -> Vec<Header> {
    vec![Header::new("token", token), Header::new("file_id", file_id)]
}

/// `GET file/info`. `Ok(Some(_))` is online with metadata, `Ok(None)` a confirmed offline file
/// (a 404 is trusted on this endpoint), `Err` anything else.
async fn file_info_status<H: PluginHost>(
    host: &H,
    token: &str,
    file_id: &str,
) -> Result<Option<api::FileEntry>, Failure> {
    match api_call::<H, api::FileInfoResult>(host, "file/info", file_query(token, file_id), true)
        .await
    {
        Ok(result) => result.file.map(Some).ok_or_else(invalid_response),
        Err(failure) if failure.kind == FailureKind::Offline => Ok(None),
        Err(failure) => Err(failure),
    }
}

/// As [`file_info_status`], but an offline file fails: `resolve` cannot hand one back.
async fn file_info<H: PluginHost>(
    host: &H,
    token: &str,
    file_id: &str,
) -> Result<api::FileEntry, Failure> {
    file_info_status(host, token, file_id)
        .await?
        .ok_or_else(|| coded(FailureKind::Offline, messages::FILE_OFFLINE))
}

/// `GET file/download` — the raw `download_url` string.
///
/// A 404 here is **not** trusted: Rapidgator's API has a documented bug that returns a spurious
/// 404 on this endpoint, even for a file `file/info` just confirmed online, so it retries
/// instead of reporting the file gone.
async fn download_url<H: PluginHost>(
    host: &H,
    token: &str,
    file_id: &str,
) -> Result<String, Failure> {
    let result: api::DownloadResult =
        api_call(host, "file/download", file_query(token, file_id), false).await?;
    result
        .download_url
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))
}

/// `GET {API_BASE}/{path}?{query}`, decoding the `{"response":..,"status":..,"details":..}`
/// envelope and classifying any error. Falls back to a bare HTTP-status classification when the
/// body is not the expected JSON shape at all. `trust_404` is JD's `trustError404`.
async fn api_call<H: PluginHost, T: DeserializeOwned>(
    host: &H,
    path: &str,
    query: Vec<Header>,
    trust_404: bool,
) -> Result<T, Failure> {
    let response = host
        .http(HttpRequest {
            method: "GET".to_owned(),
            url: format!("{}/{path}", api::API_BASE),
            query,
            headers: Vec::new(),
            body: Vec::new(),
        })
        .await?;
    let envelope: api::Envelope<T> = match serde_json::from_slice(&response.body) {
        Ok(envelope) => envelope,
        Err(_) => {
            api::ensure_http_status(response.status, trust_404).map_err(convert_failure)?;
            return Err(invalid_response());
        }
    };
    if let Some(failure) =
        api::error_from_envelope(envelope.status, envelope.details.as_deref(), trust_404)
    {
        return Err(convert_failure(failure));
    }
    envelope.response.ok_or_else(invalid_response)
}

async fn require_secret<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, crate::PASSWORD_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::PASSWORD_MISSING));
    }
    Ok(())
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

//! Keep2Share's protocol logic, written once for both builds.
//!
//! Session-based: every invocation logs in for a fresh `auth_token`, because a WebAssembly guest
//! is stateless and there is nowhere to cache one. The account-less path is different again — an
//! image captcha and a server-side countdown, both mediated by the host.

mod free;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, Label, LinkCheck, LinkStatus,
    PluginHost, ResolveInput, Resolved,
};
use serde::de::DeserializeOwned;
use url::Url;

use crate::{api, messages};

/// What an API round trip can fail with: a host-level problem (transport, budget, a rejected
/// captcha hand-off) already in the host's shape, or an error the API itself reported and
/// [`api`] classified. The free flow reinterprets the second kind — a limit becomes an IP block,
/// a captcha demand is not a failure at all — so it must not be flattened before it gets there.
pub(crate) enum CallError {
    Host(Failure),
    Api(api::ApiFailure),
}

impl From<Failure> for CallError {
    fn from(failure: Failure) -> Self {
        Self::Host(failure)
    }
}

impl From<api::ApiFailure> for CallError {
    fn from(failure: api::ApiFailure) -> Self {
        Self::Api(failure)
    }
}

impl From<CallError> for Failure {
    fn from(error: CallError) -> Self {
        match error {
            CallError::Host(failure) => failure,
            CallError::Api(failure) => convert_failure(failure),
        }
    }
}

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

/// What the account is worth, from `accountinfo`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let token = require_token(host).await?;
    let info: api::AccountInfoResult =
        api_call(host, "accountinfo", api::accountinfo_body(&token)).await?;
    let premium = api::is_premium(info.account_expires.as_ref());
    Ok(Account {
        valid: true,
        premium,
        label: Label::new()
            .premium_until(api::premium_until(premium, info.account_expires.as_ref()).as_deref())
            .into(),
        traffic_left: api::traffic_left(info.available_traffic.as_ref()),
    })
}

/// Resolves through `geturl` with an account, through the free flow without one.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed =
        Url::parse(&request.url).map_err(|error| convert_failure(api::invalid_url(&error)))?;
    let file_id = api::file_id(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?
        .to_owned();
    let Some(account_id) = request.account_id.as_deref() else {
        return free::resolve(host, &file_id).await;
    };
    require_secret(host, account_id).await?;
    let token = require_token(host).await?;
    let result: api::GetUrlResult =
        api_call(host, "geturl", api::geturl_body(&file_id, &token)).await?;
    let raw_url = result
        .url
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&raw_url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        // `/geturl` never returns a filename or size on any path; the caller learns those from a
        // separate `check()` call.
        file_name: None,
        size: None,
        headers: Vec::new(),
        checksum: None,
    })
}

/// Batched availability check.
///
/// `/getfilesinfo` is genuinely unauthenticated — JD calls it with `account = null` — so this
/// needs neither an account nor the secret gate. Every entry starts `Unknown`; a chunk that
/// answers overwrites its own entries, and a chunk whose request fails leaves them `Unknown`
/// rather than letting them read as offline.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    /// JD chunks at 100 file ids per call and loops.
    const CHUNK_SIZE: usize = 100;

    let mut results: Vec<LinkCheck> = request.urls.iter().map(|url| unknown(url)).collect();
    let indexed_ids: Vec<(usize, String)> = request
        .urls
        .iter()
        .enumerate()
        .filter_map(|(index, url)| {
            Url::parse(url)
                .ok()
                .as_ref()
                .and_then(api::file_id)
                .map(|fuid| (index, fuid.to_owned()))
        })
        .collect();

    for chunk in indexed_ids.chunks(CHUNK_SIZE) {
        let ids: Vec<&str> = chunk.iter().map(|(_, fuid)| fuid.as_str()).collect();
        let files = match api_call::<H, api::GetFilesInfoResult>(
            host,
            "getfilesinfo",
            api::getfilesinfo_body(&ids),
        )
        .await
        {
            Ok(result) => result.files,
            Err(_) => continue,
        };
        for (index, fuid) in chunk {
            results[*index] = check_one(&request.urls[*index], fuid, &files);
        }
    }
    Ok(results)
}

/// One `check()` entry, matched by id or `requested_id` the way JD's `checkLinks` does. An id
/// absent from the response is offline.
fn check_one(url: &str, fuid: &str, files: &[api::FileEntry]) -> LinkCheck {
    match files.iter().find(|entry| entry.matches_fuid(fuid)) {
        Some(entry) if entry.is_online() => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Online,
            file_name: entry.name.clone(),
            size: entry.size.and_then(|size| u64::try_from(size).ok()),
        },
        Some(_) | None => LinkCheck {
            url: url.to_owned(),
            status: LinkStatus::Offline,
            file_name: None,
            size: None,
        },
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

/// `POST /login` — a fresh session token every invocation; never cached, because there is
/// nowhere to cache it.
async fn require_token<H: PluginHost>(host: &H) -> Result<String, Failure> {
    let result: api::LoginResult = api_call(host, "login", api::login_body()).await?;
    result
        .auth_token
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_TOKEN))
}

async fn api_call<H: PluginHost, T: DeserializeOwned>(
    host: &H,
    path: &str,
    body: Vec<u8>,
) -> Result<T, Failure> {
    api_call_raw(host, path, body).await.map_err(Failure::from)
}

/// [`api_call`] without the conversion to the host's failure type. The free flow needs the
/// classified [`api::ApiFailure`] itself.
pub(crate) async fn api_call_raw<H: PluginHost, T: DeserializeOwned>(
    host: &H,
    path: &str,
    body: Vec<u8>,
) -> Result<T, CallError> {
    let response = host
        .http(
            HttpRequest {
                method: "POST".to_owned(),
                url: format!("{}/{path}", api::API_BASE),
                query: Vec::new(),
                headers: Vec::new(),
                body,
            }
            .with_header("Content-Type", api::CONTENT_TYPE_JSON),
        )
        .await?;
    let probe: api::ErrorProbe = match serde_json::from_slice(&response.body) {
        Ok(probe) => probe,
        Err(_) => {
            api::ensure_http_status(response.status)?;
            return Err(api::invalid_response().into());
        }
    };
    if let Some(failure) = api::error_from_probe(&probe) {
        return Err(failure.into());
    }
    serde_json::from_slice(&response.body).map_err(|_| api::invalid_response().into())
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
        api::ErrorKind::IpBlocked(seconds) => FailureKind::IpBlocked(seconds),
        api::ErrorKind::CaptchaFailed => FailureKind::CaptchaFailed,
    };
    let mut built = Failure::coded(kind, failure.code, failure.message);
    for (name, value) in failure.params {
        built = built.with_param(name, value);
    }
    built
}

pub(crate) fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

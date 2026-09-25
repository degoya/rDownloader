//! AllDebrid's protocol logic, written once for both builds.
//!
//! A multihoster with a Bearer-authenticated JSON API and nothing else: no cookies, no captcha,
//! no HTML. `crate::api` already held the parts that touch no host — the envelope shape, the
//! error classification, the hoster catalogue merge — so what moves here is the sequence of
//! calls that used to exist once per build.

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

/// What the account is worth, from `/user`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let response = call(host, "GET", "/user", Vec::new()).await?;
    let envelope: api::Envelope<api::UserData> = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(&envelope.status, envelope.error.as_ref()) {
        return Err(convert_failure(failure));
    }
    let data = envelope.data.ok_or_else(invalid_response)?;
    Ok(Account {
        valid: true,
        premium: data.user.is_premium,
        label: Label::new().user(data.user.username.as_deref()).into(),
        traffic_left: None,
    })
}

/// Unlocks one link, probing `/link/delayed` once when the API says the file is not fetchable
/// yet. JD sleeps in a loop there; the scheduler's own re-resolve stands in for that, so this
/// probes exactly once per invocation and reports a retry instead of holding the slot.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id).await?;
    let response = call(host, "POST", "/link/unlock", api::unlock_body(&request.url)).await?;
    let envelope: api::Envelope<api::UnlockData> = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(&envelope.status, envelope.error.as_ref()) {
        return Err(convert_failure(failure));
    }
    let data = envelope.data.ok_or_else(invalid_response)?;

    if let Some(delayed_id) = data.delayed_id() {
        let probe = call(
            host,
            "POST",
            "/link/delayed",
            api::delayed_body(&delayed_id),
        )
        .await?;
        let probe_envelope: api::Envelope<api::DelayedData> = parse_json(&probe)?;
        if let Some(failure) =
            api::error_from_status(&probe_envelope.status, probe_envelope.error.as_ref())
        {
            return Err(convert_failure(failure));
        }
        let status = probe_envelope
            .data
            .and_then(|probe_data| probe_data.status)
            .unwrap_or(3);
        match status {
            // 2 = available: the CDN URL from the initial `link/unlock` call already stands;
            // `delayed` only gates when the underlying file becomes fetchable from it.
            2 => {}
            1 => {
                return Err(coded(
                    FailureKind::Transient(Some(10)),
                    messages::LINK_DELAYED,
                ));
            }
            _ => {
                return Err(coded(
                    FailureKind::Transient(Some(300)),
                    messages::TEMPORARILY_UNAVAILABLE,
                ));
            }
        }
    }

    let raw_url = data
        .link
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&raw_url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: data.filename,
        size: data.filesize,
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
    let response = call(host, "GET", "/user/hosts", Vec::new()).await?;
    let envelope: api::Envelope<api::HostsData> = parse_json(&response)?;
    if let Some(failure) = api::error_from_status(&envelope.status, envelope.error.as_ref()) {
        return Err(convert_failure(failure));
    }
    Ok(api::merge_hosters(envelope.data.unwrap_or_default()))
}

/// AllDebrid has no link-check endpoint; unlocking is the only way to learn anything.
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

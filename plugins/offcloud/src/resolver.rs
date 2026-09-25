//! Offcloud's protocol logic, written once for both builds.
//!
//! Four calls and no loop: the account, one link, the catalogue, and the refusal that says
//! link checking is not on offer. `crate::api` holds everything that touches no host — the
//! response shapes, the catalogue merge, the failure classification — so what lives here is
//! the sequence of requests, once rather than once per build.

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

/// What the account is worth, from `GET /api/account/info`.
///
/// Reaching the endpoint at all is the key check: Offcloud answers `NOAUTH` to a key it does
/// not know, and that is the difference between an account that is merely out of premium and
/// one whose credential has been revoked.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let response = call(host, HttpRequest::get(endpoint("/account/info"))).await?;
    let info: api::AccountInfo = parse_json(&response)?;
    let state = api::account_state(&info);
    let label = Label::new().user(api::account_name(&info));
    let label = match state {
        api::AccountState::Usable => label.premium_until(info.expiration_date.as_deref()),
        api::AccountState::Free => label.premium_expired(),
        // Premium and refused: saying "premium until <date>" beside it would be true and
        // useless. The refusal is the fact that matters, and it has its own code.
        api::AccountState::Blocked => label.part(plugin_common::LabelPart::coded(
            messages::DOWNLOAD_BLOCKED.0,
            messages::DOWNLOAD_BLOCKED.1,
        )),
    };
    Ok(Account {
        // The key answered, so it is a key. What the plan covers is the other two fields.
        valid: true,
        premium: state == api::AccountState::Usable,
        label: label.into(),
        traffic_left: None,
    })
}

/// Hands the link to `POST /api/instant` and takes the address it answers with.
///
/// Asked afresh every time, which is the whole renewal story for a short-lived Offcloud
/// address: nothing is remembered between calls, so a queue that comes back to a link later
/// receives a link that was minted just now rather than one that has since expired.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id).await?;
    let body = api::form_body(&[("url", &request.url)]);
    let response = call(host, form_post(endpoint("/instant"), body)).await?;
    let answer: api::InstantDownload = parse_json(&response)?;
    let raw = answer
        .url
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    // The address comes back from the provider and goes out again as a request, so it is
    // checked rather than trusted: anything that is not an http(s) address would be a fetch
    // of something the queue never agreed to.
    if !matches(&raw) {
        return Err(coded(FailureKind::Permanent, messages::BAD_DOWNLOAD_URL));
    }
    Ok(Resolved {
        url: raw,
        file_name: answer.file_name,
        size: answer.size,
        headers: Vec::new(),
        checksum: None,
    })
}

/// The hosters this account's plan covers, as `GET /api/sites` reports them.
pub(crate) async fn hosters<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Vec<String>, Failure> {
    require_secret(host, account_id).await?;
    let response = call(host, HttpRequest::get(endpoint("/sites"))).await?;
    let entries: Vec<api::SiteEntry> = parse_json(&response)?;
    Ok(api::merge_hosters(entries))
}

/// Offcloud has no link-status endpoint for hoster links: `POST /api/cache` answers about
/// BitTorrent content and nothing else, and `/api/instant` spends an allowance to find out.
/// Saying so is better than starting a download in order to report on it.
pub(crate) async fn check<H: PluginHost>(
    _host: &H,
    _request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    Err(coded(FailureKind::Unsupported, messages::CHECK_UNSUPPORTED))
}

fn endpoint(path: &str) -> String {
    format!("{}{path}", api::API_BASE)
}

fn form_post(url: String, body: Vec<u8>) -> HttpRequest {
    HttpRequest::post(url, body).with_header("Content-Type", "application/x-www-form-urlencoded")
}

/// Makes one request with the Bearer-authenticated headers and turns every refusal, in either
/// of Offcloud's two shapes, into one classified failure.
///
/// The key never enters the plugin: `{{secret:…}}` is expanded by the host.
async fn call<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let request = request
        .with_header(
            "Authorization",
            format!("Bearer {{{{secret:{}}}}}", api::API_KEY_REFERENCE),
        )
        .with_header("Accept", "application/json");
    let response = host.http(request).await?;
    let retry_after = api::retry_after_seconds(response.header("Retry-After"));
    // A refusal decides whatever the status says, and a status decides when there is no
    // document to read. Both directions matter: Offcloud answers refusals with a 200.
    let envelope = api::error_envelope(&response.body);
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(convert_failure(failure));
    }
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
        api::ErrorKind::AccountInvalid => FailureKind::AccountInvalid,
        api::ErrorKind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
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

fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

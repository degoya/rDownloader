//! Pixeldrain's protocol logic, written once for both builds.
//!
//! Every function takes the host as a parameter rather than reaching for one, so the same code
//! runs against the native `ResolverHost` and against the WIT imports; `native.rs` and
//! `guest.rs` hold nothing but type conversions.
//!
//! Two requests make a resolve, in this order and for a reason:
//!
//! 1. `GET /api/file/{id}/info` -- the file's name, size and digest, and whether it still
//!    exists. First, so a deleted file is reported as deleted rather than as a spent quota.
//! 2. `GET /api/misc/rate_limits` -- whether this connection may fetch anything at all right
//!    now. The bytes are fetched by the transfer engine, not from in here, so a limit that is
//!    already spent has to become a scheduled wait at this point or it arrives as a 429 in the
//!    middle of a transfer instead. It is a refusal with a stable code, never a workaround: no
//!    retry loop, no second address, no other route.
//!
//! What comes out is `https://pixeldrain.com/api/file/{id}?download`, which carries no
//! signature and no deadline. That is what makes the expiry trap a non-problem here rather than
//! something worked around: a job may wait an hour in the queue and the address still works.
//!
//! **An account is optional (RD-120-38).** Without one, everything above runs exactly as it
//! always did. With one that holds an API key, every request carries
//! `Basic {{basic:pixeldrain_api_key}}` -- the host builds the pair with an empty user name,
//! which this provider's row allows -- so the quota answer is the account's rather than the
//! connection's. The transfer itself gets the same pair from the download engine, because the
//! manifest declares `transfer_auth = "basic"`; nothing here states it as a download header,
//! because a plugin never holds the key.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, LinkCheck, LinkStatus,
    PluginHost, ResolveInput, Resolved,
};
use serde::Deserialize;

use crate::{api, messages};

/// Most links one `check` call reads, so a pasted list cannot turn into an unbounded number of
/// requests against a service that rate-limits by IP.
const MAX_CHECKS: usize = 50;

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    api::file_id(url).is_some()
}

/// Hoster domains a download can come from. A single hoster serves its own, so neither the host
/// nor the account changes the answer.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(vec![api::HOST.to_owned()])
}

/// The key's vault reference, the one `[provider]` declares.
const API_KEY_REFERENCE: &str = "pixeldrain_api_key";

/// What goes into `Authorization` when an account holds a key. The host replaces the marker
/// with `base64(":<key>")`; the plugin never sees either half.
const AUTHORIZATION_TEMPLATE: &str = "Basic {{basic:pixeldrain_api_key}}";

/// What the account behind an API key is, from `GET /api/user`.
///
/// Valid when Pixeldrain accepts the key; premium when it names a paid subscription. A key that
/// is not accepted comes back as `pixeldrain.api_key_invalid`, and an account with no key
/// stored is refused before a request goes out.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    if authorization(host, Some(account_id)).await.is_none() {
        return Err(coded(FailureKind::AuthRequired, messages::API_KEY_MISSING));
    }
    let response = call(
        host,
        HttpRequest::get(api::user_url()),
        Some(AUTHORIZATION_TEMPLATE),
    )
    .await?;
    let user: api::User = parse_json(&response)?;
    Ok(Account {
        valid: true,
        premium: api::user_is_premium(&user),
        label: Vec::new(),
        traffic_left: None,
    })
}

/// The `Authorization` template for this resolve, or `None` for the free route.
///
/// `None` without an account, and `None` for an account with no key stored: sending the marker
/// then would make the host refuse the request, and a person who created an empty account is
/// better served by the free route than by a failure.
async fn authorization<H: PluginHost>(host: &H, account_id: Option<&str>) -> Option<&'static str> {
    let account_id = account_id.filter(|value| !value.is_empty())?;
    host.secret_available(account_id, API_KEY_REFERENCE)
        .await
        .then_some(AUTHORIZATION_TEMPLATE)
}

/// Turns a Pixeldrain address into a download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let id = claim(&request.url)?;
    let auth = authorization(host, request.account_id.as_deref()).await;
    let info = file_info(host, &id, auth).await?;
    // A file that exists but is behind an obstacle: a moderation block, a captcha, an allowance
    // the file itself has spent. Each carries its own code from `availability`, which is finer
    // than anything the status line says.
    if let Some(failure) = api::availability_failure(&info) {
        return Err(convert_failure(failure));
    }
    quota_gate(host, auth).await?;
    let url = api::download_url(&id);
    // Built here rather than taken from an answer, so this can only fail if a future version
    // starts following the provider's redirects. It is checked anyway: an address that leaves
    // the manifest's download domains is a fetch the queue never agreed to.
    if !api::is_download_target(&url) {
        return Err(coded(FailureKind::Permanent, messages::DIRECT_LINK_FOREIGN));
    }
    Ok(Resolved {
        url,
        file_name: api::file_name(&info),
        size: info.size,
        headers: Vec::new(),
        checksum: api::checksum(&info),
    })
}

/// Reads each address's metadata and reports whether the file is still there.
///
/// One request per link: Pixeldrain has no batch endpoint for file metadata, and inventing one
/// out of the list endpoint would ask about something else. An address this plugin does not
/// claim is reported `Unknown` rather than refused, so one foreign link in a pasted block does
/// not end the check for the rest.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let auth = authorization(host, request.account_id.as_deref()).await;
    let mut results = Vec::with_capacity(request.urls.len());
    for url in request.urls.iter().take(MAX_CHECKS) {
        results.push(check_one(host, url, auth).await);
    }
    // Anything past the cap is answered honestly rather than silently dropped: the row keeps
    // its place in the batch and says the check did not reach it.
    for url in request.urls.iter().skip(MAX_CHECKS) {
        results.push(unknown(url));
    }
    Ok(results)
}

async fn check_one<H: PluginHost>(host: &H, url: &str, auth: Option<&str>) -> LinkCheck {
    let Some(id) = api::file_id(url) else {
        return unknown(url);
    };
    match file_info(host, &id, auth).await {
        Ok(info) => LinkCheck {
            url: url.to_owned(),
            // `availability` holds states this project's enum cannot express (RD-120-36). The
            // collapse is decided in one place, `api::availability_is_offline`: only a
            // moderation block counts as gone, because a captcha or a spent allowance is a file
            // that exists and a wait, not a dead link.
            status: if api::availability_is_offline(&info) {
                LinkStatus::Offline
            } else {
                LinkStatus::Online
            },
            file_name: api::file_name(&info),
            size: info.size,
        },
        Err(failure) => LinkCheck {
            url: url.to_owned(),
            // Only "the provider says this file is gone" is offline. Every other refusal --
            // a spent allowance, an outage, an unknown token -- says nothing about the file,
            // and reporting those as offline would delete rows that are perfectly good.
            status: if failure.kind == FailureKind::Offline {
                LinkStatus::Offline
            } else {
                LinkStatus::Unknown
            },
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

/// The file identifier in an address this plugin serves.
fn claim(url: &str) -> Result<String, Failure> {
    if url::Url::parse(url).is_err() {
        return Err(coded(FailureKind::Permanent, messages::INVALID_LINK));
    }
    // `Unsupported` rather than `Permanent`: the selection walks past it and lets another
    // plugin -- the list crawler, for one -- have the address instead of ending the link here.
    api::file_id(url).ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))
}

async fn file_info<H: PluginHost>(
    host: &H,
    id: &str,
    auth: Option<&str>,
) -> Result<api::FileInfo, Failure> {
    let response = call(host, HttpRequest::get(api::info_url(id)), auth).await?;
    parse_json(&response)
}

/// Refuses before the queue is handed anything when the connection's allowance is already gone.
///
/// A quota answer that cannot be read is deliberately **not** a refusal: this is a courtesy
/// check in front of a download that would otherwise simply fail later, and letting an
/// unreadable answer block a file that is fine would be worse than the 429 it guards against.
///
/// With a key the answer is the account's own allowance, which is what a premium subscription
/// changes; without one it is the connection's.
async fn quota_gate<H: PluginHost>(host: &H, auth: Option<&str>) -> Result<(), Failure> {
    let response = call(host, HttpRequest::get(api::rate_limits_url()), auth).await?;
    let Ok(limits) = serde_json::from_slice::<api::RateLimits>(&response.body) else {
        host.log("debug", "pixeldrain rate limits could not be read");
        return Ok(());
    };
    match api::quota_failure(&limits) {
        Some(failure) => Err(convert_failure(failure)),
        None => Ok(()),
    }
}

/// Makes one request and turns every refusal, in either of Pixeldrain's two shapes, into one
/// classified failure.
async fn call<H: PluginHost>(
    host: &H,
    request: HttpRequest,
    auth: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let mut request = request.with_header("Accept", "application/json");
    if let Some(template) = auth {
        request = request.with_header("Authorization", template);
    }
    let response = host.http(request).await?;
    let retry_after = api::retry_after_seconds(response.header("Retry-After"));
    let envelope = api::error_envelope(&response.body);
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(convert_failure(failure));
    }
    Ok(response)
}

fn convert_failure(failure: api::ApiFailure) -> Failure {
    let kind = match failure.kind {
        api::ErrorKind::Transient(seconds) => FailureKind::Transient(seconds),
        api::ErrorKind::Permanent => FailureKind::Permanent,
        api::ErrorKind::Offline => FailureKind::Offline,
        api::ErrorKind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
        api::ErrorKind::IpBlocked(seconds) => FailureKind::IpBlocked(seconds),
        api::ErrorKind::AuthRequired => FailureKind::AuthRequired,
        api::ErrorKind::AccountInvalid => FailureKind::AccountInvalid,
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

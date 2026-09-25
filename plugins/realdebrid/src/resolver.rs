//! Real-Debrid's protocol logic, written once for both builds.
//!
//! A multihoster with a Bearer-authenticated JSON API. `crate::api` holds everything that
//! touches no host — the answer shapes, the error classification, the catalogue merge — and
//! what lives here is the sequence of calls.
//!
//! One thing is worth saying out loud because it is the difference between this plugin and the
//! four beside it: the Bearer token is **not** something the person typed. It is what the
//! device sign-in in `plugins/realdebrid-auth/` produced, and it expires. So a 401 here does
//! not mean "the person mistyped their key"; it means the renewal has not caught up yet, or
//! the sign-in was revoked. It is reported as `AccountInvalid` either way, because that is what
//! makes the interface offer a sign-in rather than a retry.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};
use serde::Deserialize;

use crate::{api, messages};

/// Whether this plugin claims `url`. A multihoster claims by account catalogue rather than by
/// host, so anything fetchable over http(s) is a candidate.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| api::matches(url.scheme()))
}

/// What the account is worth, from `GET /user`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_token(host, account_id).await?;
    let response = call(host, "GET", "/user", Vec::new(), true).await?;
    let user: api::UserInfo = parse_json(&response)?;
    Ok(Account {
        valid: true,
        premium: api::is_premium(user.account_type.as_deref(), user.premium),
        label: Label::new()
            .user(api::account_name(
                user.username.as_deref(),
                user.email.as_deref(),
            ))
            .into(),
        // The API states no remaining traffic in bytes anywhere, and `points` counts loyalty
        // points. Inventing a figure from it would be a number the interface formats as bytes.
        traffic_left: None,
    })
}

/// Hands the link to `POST /unrestrict/link` and takes the generated address it answers with.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_token(host, account_id).await?;
    let response = call(
        host,
        "POST",
        "/unrestrict/link",
        api::link_body(&request.url),
        true,
    )
    .await?;
    let unrestricted: api::UnrestrictedLink = parse_json(&response)?;
    // `download` and not `link`: the second is the address that went in, echoed back, and
    // queueing that would fetch the hoster's page instead of the file.
    let raw = unrestricted
        .download
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&raw).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: unrestricted.filename,
        size: unrestricted.filesize,
        headers: Vec::new(),
        // Real-Debrid's `crc` is a flag saying whether it checked, not a digest, so there is
        // nothing here a transfer could verify against.
        checksum: None,
    })
}

/// The hosters this account's plan covers, as the provider reports them.
pub(crate) async fn hosters<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Vec<String>, Failure> {
    require_token(host, account_id).await?;
    // `hosts/domains` takes no token, so the request carries none: a catalogue is public, and
    // sending a credential to fetch it would be spending one for nothing.
    let response = call(host, "GET", "/hosts/domains", Vec::new(), false).await?;
    let domains: Vec<String> = parse_json(&response)?;
    Ok(api::merge_hosters(domains))
}

/// Asks `POST /unrestrict/check` about each address, one request per link.
///
/// Three decisions worth naming:
///
/// - **The batch is bounded.** Everything past [`api::CHECK_LIMIT`] comes back `Unknown`,
///   because one request per link against an API capped at 250 a minute would rate-limit the
///   very account the check is for.
/// - **A refusal about one link is not a failed batch.** A link the provider calls unavailable
///   is `Offline`, anything else it refuses is `Unknown`, and the remaining links are still
///   asked about. Only a rate limit stops the run, because carrying on would deepen it.
/// - **`supported` is not `online`.** A file that is there on a hoster Real-Debrid does not
///   cover is still there; it just cannot be fetched through this account.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let mut results = Vec::with_capacity(request.urls.len());
    for (index, url) in request.urls.iter().enumerate() {
        if index >= api::CHECK_LIMIT {
            results.push(unknown(url));
            continue;
        }
        match call(
            host,
            "POST",
            "/unrestrict/check",
            api::link_body(url),
            false,
        )
        .await
        {
            Ok(response) => results.push(match parse_json::<api::CheckedLink>(&response) {
                Ok(checked) => LinkCheck {
                    url: url.clone(),
                    status: LinkStatus::Online,
                    file_name: checked.filename,
                    size: checked.filesize,
                },
                Err(_) => unknown(url),
            }),
            Err(failure) if matches!(failure.kind, FailureKind::RateLimited(_)) => {
                return Err(failure);
            }
            Err(failure) => results.push(LinkCheck {
                url: url.clone(),
                status: match failure.kind {
                    FailureKind::Offline => LinkStatus::Offline,
                    _ => LinkStatus::Unknown,
                },
                file_name: None,
                size: None,
            }),
        }
    }
    Ok(results)
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}

/// Calls `{API_BASE}{path}` and turns whatever came back into a failure or a response.
///
/// `authenticated` decides whether the Bearer header is attached at all. Two of Real-Debrid's
/// endpoints take no token, and sending one to them would put a credential on the wire for no
/// reason — the host would allow it, which is exactly why the plugin should not ask.
///
/// The token never enters the plugin: `{{secret:…}}` is expanded by the host, towards
/// `api.real-debrid.com` and nowhere else.
async fn call<H: PluginHost>(
    host: &H,
    method: &str,
    path: &str,
    body: Vec<u8>,
    authenticated: bool,
) -> Result<HttpResponse, Failure> {
    let mut request = HttpRequest {
        method: method.to_owned(),
        url: format!("{}{path}", api::API_BASE),
        query: Vec::new(),
        headers: Vec::new(),
        body,
    }
    .with_header("Accept", "application/json");
    if authenticated {
        request = request.with_header(
            "Authorization",
            format!("Bearer {{{{secret:{}}}}}", api::TOKEN_REFERENCE),
        );
    }
    if !request.body.is_empty() {
        request = request.with_header("Content-Type", "application/x-www-form-urlencoded");
    }
    let response = host.http(request).await?;
    let retry_after = api::retry_after_seconds(response.header("Retry-After"));
    // The failure envelope and the answer share one document, so it is read as both: an
    // `error_code` inside a 200 is still a refusal, and an answer with neither is a success.
    let envelope: api::ErrorEnvelope = serde_json::from_slice(&response.body).unwrap_or_default();
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(convert_failure(failure));
    }
    Ok(response)
}

/// Fails before any request when the account holds no access token.
///
/// Unlike the pasted-key multihosters this is not "the person forgot to type something": the
/// token is written by the sign-in and replaced by the renewal sweep, so its absence means the
/// account has never been signed in or has been signed out.
async fn require_token<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, api::TOKEN_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::TOKEN_MISSING));
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
        api::ErrorKind::IpBlocked(seconds) => FailureKind::IpBlocked(seconds),
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

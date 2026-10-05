//! Real-Debrid's protocol logic, written once for both builds.
//!
//! A multihoster with a Bearer-authenticated JSON API. `crate::api` holds everything that
//! touches no host — the answer shapes, the error classification, the catalogue merge — and
//! what lives here is the sequence of calls.
//!
//! One thing is worth saying out loud because it is the difference between this plugin and the
//! four beside it: the Bearer token is one of two, by the account's mode (RD-150-09) -- the
//! access token the sign-in in `plugins/realdebrid-auth/` stored and keeps renewed, or the
//! person's private API token from real-debrid.com/apitoken. Which one is asked of the host
//! before the first request, and the plugin never learns the mode itself. A 401 means the token
//! was mistyped, renewed at Real-Debrid or revoked. It is reported as `AccountInvalid`, because
//! that is what makes the interface ask for the credential again rather than retry.

use plugin_common::failure::coded;
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
    let token = require_token(host, account_id).await?;
    let response = call(host, "GET", "/user", Vec::new(), Some(token)).await?;
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
    let token = require_token(host, account_id).await?;
    let response = call(
        host,
        "POST",
        "/unrestrict/link",
        api::link_body(&request.url),
        Some(token),
    )
    .await?;
    let unrestricted: api::UnrestrictedLink = parse_json(&response)?;
    // `download` and not `link`: the second is the address that went in, echoed back, and
    // queueing that would fetch the hoster's page instead of the file.
    let raw = unrestricted
        .download
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&raw)?;
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
    let response = call(host, "GET", "/hosts/domains", Vec::new(), None).await?;
    let domains: Vec<String> = parse_json(&response)?;
    Ok(api::merge_hosters(domains))
}

/// Answers the LinkGrabber's check from the public catalogue, without asking about any file.
///
/// `POST /unrestrict/check` is deliberately not called (1.5.3). Measured on 2026-09-28 it
/// answered `429` with `error_code` 34 to the very first request, token or not, so every check
/// came back as a rate-limit wait on links the download could fetch; and where it answers, it
/// costs one request per link against the 250 a minute the downloads share. Whether a file is
/// there is what `unrestrict/link` answers when the download starts, so nothing is lost that
/// the download does not learn anyway. The check is one `GET /hosts/domains` per batch:
///
/// - **A link on a covered hoster is `Unknown`**: Real-Debrid supports it, and whether the
///   file is there cannot be verified before the download.
/// - **A batch with no covered link fails `Unsupported`** (`realdebrid.host_unsupported`),
///   because a link status has no word for "not this provider". The LinkGrabber only routes
///   covered links here, by the same catalogue, so an uncovered link in a mixed batch is
///   `Unknown` too, and `resolve` names it when the download tries it.
/// - **A catalogue that cannot be fetched leaves every link `Unknown`**, a rate limit
///   included. A check has nothing to wait for, and a wait reported here would hold links
///   whose download is fine.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let catalogue = call(host, "GET", "/hosts/domains", Vec::new(), None)
        .await
        .and_then(|response| parse_json::<Vec<String>>(&response));
    if let Ok(domains) = catalogue {
        let hosters = api::merge_hosters(domains);
        if !request.urls.is_empty() && !request.urls.iter().any(|url| api::covers(&hosters, url)) {
            return Err(coded(FailureKind::Unsupported, messages::HOST_UNSUPPORTED));
        }
    }
    Ok(request.urls.iter().map(|url| unknown(url)).collect())
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
/// `token` names the reference the Bearer header carries, or `None` for no header at all. The
/// catalogue takes no token, and sending one to it would put a credential on the wire for no
/// reason — the host would allow it, which is exactly why the plugin should not ask.
///
/// The token never enters the plugin: `{{secret:…}}` is expanded by the host, towards
/// `api.real-debrid.com` and nowhere else.
async fn call<H: PluginHost>(
    host: &H,
    method: &str,
    path: &str,
    body: Vec<u8>,
    token: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let mut request = HttpRequest {
        method: method.to_owned(),
        url: format!("{}{path}", api::API_BASE),
        query: Vec::new(),
        headers: Vec::new(),
        body,
    }
    .with_header("Accept", "application/json");
    if let Some(reference) = token {
        request = request.with_header(
            "Authorization",
            format!("Bearer {{{{secret:{reference}}}}}"),
        );
    }
    if !request.body.is_empty() {
        request = request.with_header("Content-Type", "application/x-www-form-urlencoded");
    }
    // The failure envelope and the answer share one document, so it is read as both: an
    // `error_code` inside a 200 is still a refusal, and an answer with neither is a success.
    plugin_common::failure::call(host, request, |status, retry_after, body| {
        let envelope: api::ErrorEnvelope = serde_json::from_slice(body).unwrap_or_default();
        api::failure_from(status, retry_after, &envelope)
    })
    .await
}

/// The reference of the token this account holds, or a failure before any request when it
/// holds none.
///
/// Asked of the host rather than decided here: `secret-available` answers only for the slot the
/// account's mode makes live, so a signed-in account names the access token and an account with
/// a pasted key names that key. Neither being there means the account has never been signed in,
/// was signed out, or has no token typed yet.
async fn require_token<H: PluginHost>(host: &H, account_id: &str) -> Result<&'static str, Failure> {
    for reference in api::TOKEN_REFERENCES {
        if host.secret_available(account_id, reference).await {
            return Ok(reference);
        }
    }
    Err(coded(FailureKind::AuthRequired, messages::TOKEN_MISSING))
}

fn parse_json<T: for<'de> Deserialize<'de>>(response: &HttpResponse) -> Result<T, Failure> {
    serde_json::from_slice(&response.body)
        .map_err(|_| coded(FailureKind::Transient(None), messages::INVALID_RESPONSE))
}

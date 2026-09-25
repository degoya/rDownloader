//! LinkSnappy's protocol logic, written once for both builds.
//!
//! A multihoster whose API takes the credentials on every call rather than through a session:
//! JD establishes a cookie session with `AUTHENTICATE` and then calls the endpoints bare, but
//! this plugin has no session to rely on, so `username`/`password` travel with each request as
//! `{{username}}`/`{{secret:…}}` markers the host expands.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse, Label, LinkCheck,
    PluginHost, ResolveInput, Resolved,
};
use serde::de::DeserializeOwned;

use crate::{api, messages};

/// Whether this plugin claims `url`. A multihoster claims by account catalogue rather than by
/// host, so anything fetchable over http(s) is a candidate.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| api::matches(url.scheme()))
}

/// What the account is worth, from `USERDETAILS`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id).await?;
    let details: api::UserDetails =
        api_call(host, "USERDETAILS", credential_query(), false).await?;
    Ok(Account {
        valid: true,
        premium: api::is_premium(details.expire.as_ref()),
        label: match api::subscription(details.expire.as_ref()) {
            api::Subscription::Lifetime => Label::new().premium_lifetime(),
            api::Subscription::Expired => Label::new().premium_expired(),
            api::Subscription::Elite | api::Subscription::Unknown => Label::new(),
        }
        .into(),
        traffic_left: api::traffic_left(details.trafficleft.as_ref()),
    })
}

/// Generates the direct link through `linkgen`, which answers with a per-link envelope of its
/// own — so the outer status and the entry's status are both classified.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_secret(host, account_id).await?;
    let response = get(
        host,
        "linkgen",
        vec![Header::new("genLinks", api::gen_links_json(&request.url))],
    )
    .await?;
    let parsed: api::GenLinksResponse = match serde_json::from_slice(&response.body) {
        Ok(parsed) => parsed,
        Err(_) => {
            api::ensure_http_status(response.status).map_err(convert_failure)?;
            return Err(invalid_response());
        }
    };
    if let Some(failure) =
        api::error_from_envelope(parsed.status.as_deref(), parsed.error.as_ref(), true)
    {
        return Err(convert_failure(failure));
    }
    let Some(entry) = parsed.links.and_then(|links| links.into_iter().next()) else {
        return Err(coded(
            FailureKind::Transient(None),
            messages::NO_DOWNLOAD_URL,
        ));
    };
    if let Some(failure) =
        api::error_from_envelope(entry.status.as_deref(), entry.error.as_ref(), true)
    {
        return Err(convert_failure(failure));
    }
    let raw_url = entry
        .generated
        .filter(|url| !url.is_empty())
        .ok_or_else(|| coded(FailureKind::Transient(None), messages::NO_DOWNLOAD_URL))?;
    let url = api::parse_download_url(&raw_url).map_err(convert_failure)?;
    Ok(Resolved {
        url: url.to_string(),
        file_name: entry.filename,
        size: entry.size,
        headers: Vec::new(),
        checksum: None,
    })
}

/// The hosters this account's plan covers, from `FILEHOSTS`.
///
/// JD requests this endpoint bare, relying on the cookie session `loginAPI` just established.
/// This plugin has no such session, so the credentials travel with the call and it is gated the
/// same way every other endpoint is.
pub(crate) async fn hosters<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Vec<String>, Failure> {
    require_secret(host, account_id).await?;
    let response = get(host, "FILEHOSTS", credential_query()).await?;
    let envelope: api::Envelope<std::collections::HashMap<String, api::HostEntry>> =
        match serde_json::from_slice(&response.body) {
            Ok(envelope) => envelope,
            Err(_) => {
                api::ensure_http_status(response.status).map_err(convert_failure)?;
                return Err(invalid_response());
            }
        };
    if let Some(failure) =
        api::error_from_envelope(envelope.status.as_deref(), envelope.error.as_ref(), false)
    {
        return Err(convert_failure(failure));
    }
    Ok(api::merge_hosters(envelope.value.unwrap_or_default()))
}

/// LinkSnappy has no link-check endpoint; generating the link is the only way to learn anything.
pub(crate) async fn check<H: PluginHost>(
    _host: &H,
    _request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    Err(coded(FailureKind::Unsupported, messages::CHECK_UNSUPPORTED))
}

/// `username`/`password`, as the markers the host expands. Neither value enters the plugin.
fn credential_query() -> Vec<Header> {
    vec![
        Header::new("username", "{{username}}"),
        Header::new(
            "password",
            format!("{{{{secret:{}}}}}", api::PASSWORD_REFERENCE),
        ),
    ]
}

/// `GET {API_BASE}/{path}?{query}`, decoding the `{"status":..,"error":..,"return":..}` envelope
/// and classifying any error. Falls back to a bare HTTP-status classification when the body is
/// not the expected JSON shape at all.
async fn api_call<H: PluginHost, T: DeserializeOwned>(
    host: &H,
    path: &str,
    query: Vec<Header>,
    has_link: bool,
) -> Result<T, Failure> {
    let response = get(host, path, query).await?;
    let envelope: api::Envelope<T> = match serde_json::from_slice(&response.body) {
        Ok(envelope) => envelope,
        Err(_) => {
            api::ensure_http_status(response.status).map_err(convert_failure)?;
            return Err(invalid_response());
        }
    };
    if let Some(failure) = api::error_from_envelope(
        envelope.status.as_deref(),
        envelope.error.as_ref(),
        has_link,
    ) {
        return Err(convert_failure(failure));
    }
    envelope.value.ok_or_else(invalid_response)
}

async fn get<H: PluginHost>(
    host: &H,
    path: &str,
    query: Vec<Header>,
) -> Result<HttpResponse, Failure> {
    host.http(HttpRequest {
        method: "GET".to_owned(),
        url: format!("{}/{path}", api::API_BASE),
        query,
        headers: Vec::new(),
        body: Vec::new(),
    })
    .await
}

/// Fails before any request when the account has no password: every flow needs one.
async fn require_secret<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, api::PASSWORD_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::PASSWORD_MISSING));
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

fn invalid_response() -> Failure {
    coded(FailureKind::Transient(None), messages::INVALID_RESPONSE)
}

fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

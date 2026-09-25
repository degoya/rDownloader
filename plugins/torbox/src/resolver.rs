//! TorBox's protocol logic, written once for both builds.
//!
//! Two endpoints and no more. `GET /user/me` says what the account is worth, and
//! `GET /{kind}/requestdl` turns a finished job's file into the address the bytes come from.
//!
//! The one thing worth saying out loud, because it is the whole reason this plugin exists
//! rather than the remote-job plugin handing back a finished address: **`requestdl` mints a
//! short-lived ticket, and this call is made again on every attempt.** The durable address is
//! the `requestdl` one, `worker::run` re-resolves it before every attempt and
//! `replay::before_resume` re-resolves it before reusing a partial file, so a pause of an hour
//! costs one extra request rather than a failed resume. Writing a minted address into the row
//! instead would be a ticket that is dead the next time anybody looks at it.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};
use serde::Deserialize;

use crate::{api, messages};

/// Whether this plugin claims `url`: a TorBox `requestdl` address and nothing else.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    api::matches(url)
}

/// What the account is worth, from `GET /user/me`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_key(host, account_id).await?;
    let response = call(host, "GET", "/user/me", &[], None, Vec::new()).await?;
    let user: api::UserInfo = parse_data(&response)?;
    let premium = user.is_premium();
    let mut label = Label::new().user(user.display_name());
    label = if premium {
        label.premium_until(user.premium_expires_at.as_deref())
    } else {
        label.premium_expired()
    };
    Ok(Account {
        valid: true,
        premium,
        label: label.into(),
        // TorBox counts jobs and speed rather than a byte budget, and it states no remaining
        // figure in bytes anywhere. Inventing one would be a number the interface formats as
        // bytes and nobody can act on.
        traffic_left: None,
    })
}

/// Mints the address one file is fetched from, now.
///
/// Rebuilt from the two identifiers rather than forwarded: the address arrives from a
/// candidate row a person can edit, and what goes out carries the account's key.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING))?;
    require_key(host, account_id).await?;
    let ticket = api::read_ticket(&request.url)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::NOT_A_TICKET))?;
    let response = call(
        host,
        "GET",
        ticket.kind.request_path,
        &[
            (ticket.kind.id_field, ticket.job_id.as_str()),
            ("file_id", ticket.file_id.as_str()),
        ],
        // The key travels as a query parameter because that is where `requestdl` takes it,
        // and as a marker because this plugin has no value to put there.
        Some(("token", format!("{{{{secret:{}}}}}", api::TOKEN_REFERENCE))),
        Vec::new(),
    )
    .await?;
    let url = api::read_download_address(&response.body)
        .ok_or_else(|| coded(FailureKind::Permanent, messages::NO_DOWNLOAD_URL))?;
    Ok(Resolved {
        url,
        // Deliberately nothing else. The name and the size are already on the candidate row,
        // put there by the remote job that produced this address; stating them again from a
        // second request would be one more call against the account's budget per file.
        file_name: None,
        size: None,
        headers: Vec::new(),
        checksum: None,
    })
}

/// TorBox is not a multihoster in this plugin's sense, so there is no catalogue.
///
/// It will fetch an ordinary hoster link, but only as a web-download job that takes minutes
/// and answers with files -- `plugins/torbox-jobs/` submits those. Listing hosters here would
/// tell the core this plugin can resolve links it cannot, and take them away from the plugins
/// that can.
pub(crate) async fn hosters<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Vec<String>, Failure> {
    require_key(host, account_id).await?;
    Ok(Vec::new())
}

/// Asks the job's own list whether the file behind each address is still there.
///
/// One request per address, bounded by [`api::CHECK_LIMIT`]: the polling of every running job
/// shares this account's request budget, so a batch of five hundred pasted addresses must not
/// become five hundred requests.
///
/// **A file TorBox no longer offers is `Offline`, and everything else is `Unknown`.** The
/// distinction matters because the LinkGrabber acts on `Offline`: a rate limit or an outage
/// says nothing about whether a file is there, and marking it gone would delete somebody's
/// links over a bad minute.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let mut results = Vec::with_capacity(request.urls.len());
    for (index, url) in request.urls.iter().enumerate() {
        let Some(ticket) = api::read_ticket(url).filter(|_| index < api::CHECK_LIMIT) else {
            results.push(unknown(url));
            continue;
        };
        match call(
            host,
            "GET",
            ticket.kind.list_path,
            &[("id", ticket.job_id.as_str()), ("bypass_cache", "true")],
            None,
            Vec::new(),
        )
        .await
        {
            Ok(response) => results.push(match parse_data::<api::JobEntry>(&response) {
                Ok(entry) => match entry.file(&ticket.file_id) {
                    Some(file) if entry.download_present.unwrap_or(false) => LinkCheck {
                        url: url.clone(),
                        status: LinkStatus::Online,
                        file_name: file
                            .short_name
                            .clone()
                            .or_else(|| file.name.clone())
                            .filter(|name| !name.trim().is_empty()),
                        size: file.size,
                    },
                    // The job is there and the bytes are not, which is a job still running
                    // rather than a file that is gone.
                    Some(_) => unknown(url),
                    None => LinkCheck {
                        url: url.clone(),
                        status: LinkStatus::Offline,
                        file_name: None,
                        size: None,
                    },
                },
                Err(_) => unknown(url),
            }),
            Err(failure) if matches!(failure.kind, FailureKind::RateLimited(_)) => {
                // Carrying on would deepen the very limit that refused this one.
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
/// The key never enters the plugin: `{{secret:...}}` is expanded by the host, towards
/// `api.torbox.app` and nowhere else. It travels in the `Authorization` header always, and
/// additionally as a query parameter at the one endpoint that takes it there.
async fn call<H: PluginHost>(
    host: &H,
    method: &str,
    path: &str,
    query: &[(&str, &str)],
    token_query: Option<(&str, String)>,
    body: Vec<u8>,
) -> Result<HttpResponse, Failure> {
    let mut request = HttpRequest {
        method: method.to_owned(),
        url: format!("{}{path}", api::API_BASE),
        query: Vec::new(),
        headers: Vec::new(),
        body,
    }
    .with_header("Accept", "application/json")
    .with_header(
        "Authorization",
        format!("Bearer {{{{secret:{}}}}}", api::TOKEN_REFERENCE),
    );
    for (name, value) in query {
        request = request.with_query(name, (*value).to_owned());
    }
    if let Some((name, value)) = token_query {
        request = request.with_query(name, value);
    }
    let response = host.http(request).await?;
    let retry_after = api::retry_after_seconds(response.header("Retry-After"));
    // The failure envelope and the answer share one document, so it is read as both: an
    // `error` word inside a 200 is still a refusal, and an answer with neither is a success.
    let envelope: api::ErrorEnvelope = serde_json::from_slice(&response.body).unwrap_or_default();
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(convert_failure(failure));
    }
    Ok(response)
}

/// Fails before any request when the account holds no API key.
async fn require_key<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if !host
        .secret_available(account_id, api::TOKEN_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::KEY_MISSING));
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

/// The `data` half of an answer, parsed.
fn parse_data<T: for<'de> Deserialize<'de>>(response: &HttpResponse) -> Result<T, Failure> {
    let envelope: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|_| coded(FailureKind::Transient(None), messages::INVALID_RESPONSE))?;
    let payload = envelope
        .get("data")
        .cloned()
        .ok_or_else(|| coded(FailureKind::Transient(None), messages::INVALID_RESPONSE))?;
    serde_json::from_value(payload)
        .map_err(|_| coded(FailureKind::Transient(None), messages::INVALID_RESPONSE))
}

fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

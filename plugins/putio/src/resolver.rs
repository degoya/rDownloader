//! Put.io's protocol logic, written once for both builds.
//!
//! Everything here goes through the official API v2 and nothing else: `files/{id}` for what a
//! file is, `account/info` for the account. There is no scraping of the web interface and no
//! guessing at storage hostnames — those are the tricks that break, and the ones the job's
//! cross-cutting requirement rules out.
//!
//! The account's access token is never in this file. Requests carry the marker
//! `{{secret:putio_access_token}}`, which the host expands on the way out and only towards
//! `api.put.io`; the sibling OAuth plugin is what puts a value behind it.
//!
//! One thing this resolver deliberately does *not* do is ask for a download address. Put.io
//! offers `GET /v2/files/{id}/url`, which answers with a signed storage address that expires;
//! this plugin answers with the stable per-file address instead and lets the host attach the
//! account's token to it. Why, at length: `putio_common::address`.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LabelPart,
    LinkCheck, LinkStatus, PluginHost, ResolveInput, Resolved,
};
use putio_common::{address, reason::ErrorEnvelope};

use crate::{api, messages};

/// Most links one `check` call looks up. A check is one request per link, so an unbounded
/// batch is an unbounded number of requests inside one invocation's budget — and against an
/// account's shared rate limit.
const MAX_CHECKS: usize = 50;

/// Whether this plugin claims `url`. Answered from the address alone and reaching nothing.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    address::claim(url).is_some()
}

/// The hosts this plugin serves, for the account's catalogue.
///
/// Put.io's own and nothing else: it is a hoster for its own storage and unrestricts nobody
/// else's links, so a list naming other hosters would be a promise it cannot keep.
///
/// One entry and not three. The catalogue matches a host or any subdomain of it
/// (`rd_api::hosters::supports`), so `put.io` already covers `api.put.io` and `app.put.io` —
/// and it is also the name a person recognises in the account list, which `api.put.io` is not.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(vec!["put.io".to_owned()])
}

/// What the account is, from `account/info`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_token(host, account_id).await?;
    let response = call(
        host,
        HttpRequest::get(format!("{}/account/info", address::API)),
    )
    .await?;
    let document: api::AccountResponse = parse(&response)?;
    let info = document.info.unwrap_or_default();
    let active = info.account_active.unwrap_or(false);
    Ok(Account {
        valid: active,
        // Put.io has no free tier: an account that exists and is active is a paying one, and
        // there is no second class of it that changes what this plugin may do.
        premium: active,
        label: Label::new()
            .user(info.mail.as_deref().or(info.username.as_deref()))
            .maybe(disk_free(info.disk.and_then(|disk| disk.avail)))
            .into(),
        // Deliberately not `disk.avail`. That is space left to *store* into, and reporting it
        // as remaining traffic would tell somebody with a full account that they cannot
        // download from it — which is not true, and is exactly the kind of wrong number an
        // account row is believed on sight.
        traffic_left: None,
    })
}

/// Turns one Put.io address into one download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let file_id = address::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_PUTIO_LINK, FailureKind::Unsupported))?;
    let file = fetch_file(host, file_id).await?;
    if file.is_folder() {
        // Said plainly rather than as "this file has no bytes", which is what a folder's
        // record looks like from here.
        return Err(refuse(messages::IS_A_FOLDER, FailureKind::Unsupported));
    }
    Ok(Resolved {
        // The stable API address rather than a signed one-shot URL. That is what makes a
        // resume possible: the scheduler asks this plugin again before continuing a partial
        // file, and an address whose only identity was an expiring signature would have
        // nothing left to ask for.
        url: address::download_url(file_id),
        file_name: file.name.clone().filter(|name| !name.is_empty()),
        size: file.size,
        // None. The credential this address needs is the account's own token, and the host
        // attaches that because the `putio` provider row says it may — a resolver states
        // headers as *values*, and it has no value to state.
        headers: Vec::new(),
        checksum: file.checksum(),
    })
}

/// Whether each of a batch of links is still there.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    input: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let mut results = Vec::new();
    for url in input.urls.iter().take(MAX_CHECKS) {
        let Some(file_id) = address::claim(url) else {
            results.push(unknown(url));
            continue;
        };
        results.push(match fetch_file(host, file_id).await {
            Ok(file) if file.is_folder() => LinkCheck {
                url: url.clone(),
                // A folder is there; it is simply not a download. Reporting it offline would
                // be a lie about the account's contents.
                status: LinkStatus::Unknown,
                file_name: file.name.clone().filter(|name| !name.is_empty()),
                size: None,
            },
            Ok(file) => LinkCheck {
                url: url.clone(),
                status: LinkStatus::Online,
                file_name: file.name.clone().filter(|name| !name.is_empty()),
                size: file.size,
            },
            // A file Put.io says is gone is offline; anything else says nothing about the
            // link, so it stays unknown rather than being reported as missing.
            Err(failure) if failure.code.as_deref() == Some(messages::FILE_NOT_FOUND.0) => {
                LinkCheck {
                    url: url.clone(),
                    status: LinkStatus::Offline,
                    file_name: None,
                    size: None,
                }
            }
            Err(_) => unknown(url),
        });
    }
    Ok(results)
}

/// One `GET /v2/files/{id}`.
async fn fetch_file<H: PluginHost>(host: &H, file_id: u64) -> Result<api::FileRecord, Failure> {
    let response = call(host, HttpRequest::get(address::file_url(file_id))).await?;
    let document: api::FileResponse = parse(&response)?;
    document
        .file
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// Makes one request and turns every answer that is not one into a refusal.
///
/// The whole vocabulary of "Put.io said no" lives here, so no caller decides a second time
/// what a status meant — which is how a rate limit and an expired sign-in end up under one
/// message.
async fn call<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let response = host
        .http(request.with_header(
            "Authorization",
            format!("Bearer {{{{secret:{}}}}}", api::TOKEN_REFERENCE),
        ))
        .await?;
    let envelope = ErrorEnvelope::of(&response.body);
    // Asked for only when it is needed: `now-unix-seconds` is a host call, and a reset header
    // is on the one answer in a thousand that is a rate limit.
    let reset = if response.status == 429 {
        api::rate_limit_wait(
            response.header("X-RateLimit-Reset"),
            host.now_unix_seconds().await,
        )
    } else {
        None
    };
    match api::failure_from(response.status, reset, &envelope) {
        None => Ok(response),
        Some(refusal) => {
            let mut failure = Failure::coded(refusal.kind, refusal.code, refusal.message);
            if let Some(reason) = refusal.reason {
                // Sanitised in `putio_common::reason` before it ever gets here, so an error
                // document that quoted something it should not have keeps nothing.
                failure = failure.with_param("reason", reason);
            }
            Err(failure)
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(response: &HttpResponse) -> Result<T, Failure> {
    serde_json::from_slice(&response.body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// The account's free storage, as a label part, when Put.io stated a figure.
fn disk_free(available: Option<u64>) -> Option<LabelPart> {
    let bytes = available?;
    Some(
        LabelPart::coded(messages::DISK_FREE, format!("{bytes} bytes free"))
            .with_param("bytes", bytes.to_string()),
    )
}

/// The account holds a Put.io token, or this fails before a request goes out.
///
/// Asked rather than assumed: without it the host would expand `{{secret:…}}` into nothing and
/// Put.io would answer 401, which reads as "your sign-in expired" for an account that was
/// never signed in at all.
async fn require_token<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if host
        .secret_available(account_id, api::TOKEN_REFERENCE)
        .await
    {
        Ok(())
    } else {
        Err(refuse(messages::TOKEN_MISSING, FailureKind::AuthRequired))
    }
}

fn account(account_id: Option<&str>) -> Result<&str, Failure> {
    account_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| refuse(messages::ACCOUNT_MISSING, FailureKind::AuthRequired))
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}

fn refuse((code, message): (&str, &str), kind: FailureKind) -> Failure {
    Failure::coded(kind, code, message)
}

#[cfg(test)]
mod tests {
    use super::{disk_free, matches};

    #[test]
    fn only_put_io_file_addresses_are_claimed() {
        assert!(matches("https://api.put.io/v2/files/42/download"));
        assert!(matches("https://app.put.io/files/42"));
        // A magnet is the remote-job sibling's, and a stranger's host is nobody's.
        assert!(!matches("magnet:?xt=urn:btih:da39a3ee"));
        assert!(!matches("https://ddownload.com/f/abc"));
    }

    #[test]
    fn free_space_is_a_label_part_only_when_put_io_stated_one() {
        let part = disk_free(Some(1024)).expect("a part");
        assert_eq!(part.code, "putio.disk_free");
        assert_eq!(part.params, vec![("bytes".to_owned(), "1024".to_owned())]);
        assert!(disk_free(None).is_none());
    }
}

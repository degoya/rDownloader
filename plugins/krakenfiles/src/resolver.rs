//! KrakenFiles' protocol logic, written once for both builds.
//!
//! Every function here takes the host as a parameter rather than reaching for one, so the same
//! code runs against the native `ResolverHost` and against the WIT imports. The two adapters
//! that supply it - `native.rs` and `guest.rs` - hold nothing but type conversions.
//!
//! The flow is the one JDownloader's `KrakenfilesCom` (revision 52214) and pyLoad's
//! `KrakenfilesCom` drive, measured again on 2026-09-21 (RD-103-08): the file page carries a
//! `POST /download/<hash>` form with a hidden `token`, and the site checks a Cloudflare
//! Turnstile answer before anything else - a post without one is `{"status":"error","msg":
//! "captcha not valid"}` under HTTP 500. The answer to a good post is `{"status":"ok","url":
//! ...}`; it could not be measured without a solved widget, so its shape follows the two
//! references and the link's host is held to the manifest's download domains. `fingerprint`
//! and `userdata` are posted empty, as both references post them: nothing is faked.
//!
//! Link checks go through `GET /json/<id>`, the metadata endpoint behind the site's embed
//! player: no token, no captcha, and `[]` for a file that is gone.

use plugin_common::{
    Account, CaptchaChallenge, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse,
    LinkCheck, LinkStatus, PluginHost, ResolveInput, Resolved, WidgetChallenge,
};
use serde::Deserialize;
use url::Url;

use crate::{messages, page};

/// JDownloader waits an hour after a 403, 404 or 405 on the direct link; 405 "happens with
/// too many connections", and nothing distinguishes the three from here.
const REFUSED_LINK_RETRY_SECONDS: u64 = 3600;

/// The site's own wording for a rejected Turnstile answer.
const CAPTCHA_REFUSAL: &str = "captcha not valid";

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(page::file_id)
        .is_some()
}

/// Hoster domains a download can come from. A single hoster serves its own, so neither the
/// host nor the account changes the answer.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(crate::HOSTERS
        .iter()
        .map(|host| (*host).to_owned())
        .collect())
}

/// This provider takes no account, so there is never one to check.
pub(crate) async fn check_account<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Account, Failure> {
    Err(coded(FailureKind::Unsupported, messages::NO_ACCOUNT))
}

/// Turns a link into a download through the file page's form. The account, if the request
/// carries one, is deliberately ignored: it cannot belong to this provider, which has none.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url)
        .map_err(|_| coded(FailureKind::Permanent, messages::INVALID_LINK))?;
    let id = page::file_id(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?;
    // A rejected answer is tried once more from the top - a fresh page, a fresh token, a
    // fresh challenge - the way JDownloader's framework retries a captcha exception. Exactly
    // once: a second refusal is reported, and the scheduler decides how often to come back.
    let outcome = match attempt(host, &id).await? {
        Attempt::CaptchaRejected => attempt(host, &id).await?,
        first => first,
    };
    let Attempt::Link { url, page_name } = outcome else {
        return Err(coded(
            FailureKind::CaptchaFailed,
            messages::CAPTCHA_REJECTED,
        ));
    };
    transfer(host, &url, page_name).await
}

/// Link check through the metadata endpoint, one request per link and no account.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let mut results = Vec::with_capacity(request.urls.len());
    for url in &request.urls {
        let Some(id) = Url::parse(url).ok().as_ref().and_then(page::file_id) else {
            results.push(LinkCheck {
                url: url.clone(),
                status: LinkStatus::Unknown,
                file_name: None,
                size: None,
            });
            continue;
        };
        let response = host.http(HttpRequest::get(page::json_url(&id))).await?;
        let (status, file_name, size) = metadata_verdict(&response, &id);
        results.push(LinkCheck {
            url: url.clone(),
            status,
            file_name,
            size,
        });
    }
    Ok(results)
}

/// One pass through the form: page, widget, post, answer.
enum Attempt {
    /// The site handed out a link; `page_name` is the file name the page showed, for when the
    /// transfer names none.
    Link {
        url: String,
        page_name: Option<String>,
    },
    /// The site said "captcha not valid".
    CaptchaRejected,
}

async fn attempt<H: PluginHost>(host: &H, id: &str) -> Result<Attempt, Failure> {
    let (page_response, body) = file_page(host, id).await?;
    let form = page::download_form(&body).map_err(|diagnosis| {
        Failure::coded(
            FailureKind::Permanent,
            messages::PAGE_LAYOUT_CHANGED,
            messages::page_layout_changed(&diagnosis),
        )
        .with_param("diagnosis", diagnosis)
    })?;
    let solution = host
        .solve_captcha(CaptchaChallenge::Turnstile(WidgetChallenge {
            site_key: form.site_key.clone(),
            page_url: page_response.final_url.clone(),
            invisible: false,
        }))
        .await?;
    let fields = [
        ("token".to_owned(), form.token.clone()),
        ("userdata".to_owned(), String::new()),
        ("fingerprint".to_owned(), String::new()),
        ("cf-turnstile-response".to_owned(), solution.token),
    ];
    let posted = host
        .http(
            HttpRequest::post(form.action.clone(), page::encode_form(&fields))
                .with_header("Content-Type", "application/x-www-form-urlencoded")
                .with_header("X-Requested-With", "XMLHttpRequest")
                .with_header("hash", form.hash.clone())
                .with_header("Referer", page_response.final_url.clone()),
        )
        .await?;
    Ok(match download_answer(&posted)? {
        DownloadVerdict::Link(url) => Attempt::Link {
            url,
            page_name: page::file_name(&body),
        },
        DownloadVerdict::CaptchaRejected => Attempt::CaptchaRejected,
    })
}

/// The file page and its body, or the "gone" failure the site answers instead.
async fn file_page<H: PluginHost>(host: &H, id: &str) -> Result<(HttpResponse, String), Failure> {
    let response = host.http(HttpRequest::get(page::file_page_url(id))).await?;
    let body = response.text().into_owned();
    if response.status == 404 || page::is_file_unavailable(&body) {
        return Err(coded(FailureKind::Offline, messages::FILE_UNAVAILABLE));
    }
    ensure_http_status(&response)?;
    Ok((response, body))
}

/// The JSON the download form's post answers with; every field optional, because only the
/// error shape was measured.
#[derive(Deserialize)]
struct DownloadAnswer {
    status: Option<String>,
    url: Option<String>,
    msg: Option<String>,
}

enum DownloadVerdict {
    Link(String),
    CaptchaRejected,
}

/// Reads the post's answer. The JSON comes first whatever the status - the measured refusal
/// arrives under HTTP 500 - and only a body that is not JSON is judged by its status.
fn download_answer(response: &HttpResponse) -> Result<DownloadVerdict, Failure> {
    let Ok(answer) = serde_json::from_slice::<DownloadAnswer>(&response.body) else {
        ensure_http_status(response)?;
        return Err(coded(
            FailureKind::Transient(None),
            messages::INVALID_RESPONSE,
        ));
    };
    match answer.status.as_deref() {
        Some("ok") => match answer.url.filter(|url| !url.trim().is_empty()) {
            Some(url) => Ok(DownloadVerdict::Link(url)),
            // Never the page itself: a resolve that answered with the file page would save
            // HTML under the file's name, which is the one outcome this plugin exists to
            // prevent.
            None => Err(coded(
                FailureKind::Transient(None),
                messages::DIRECT_LINK_MISSING,
            )),
        },
        Some("error") => {
            let message = sanitised(answer.msg.as_deref().unwrap_or_default());
            if message.to_ascii_lowercase().contains(CAPTCHA_REFUSAL) {
                return Ok(DownloadVerdict::CaptchaRejected);
            }
            // Anything else the site says is temporary in JDownloader's reading, and the
            // wording travels as a parameter so the person sees what the site said.
            Err(Failure::coded(
                FailureKind::Transient(None),
                messages::DOWNLOAD_REFUSED,
                messages::download_refused(&message),
            )
            .with_param("message", message))
        }
        _ => Err(coded(
            FailureKind::Transient(None),
            messages::INVALID_RESPONSE,
        )),
    }
}

/// Holds the link to the manifest's download domains and probes it, so a refused link is a
/// coded failure here rather than a stalled transfer later.
async fn transfer<H: PluginHost>(
    host: &H,
    link: &str,
    page_name: Option<String>,
) -> Result<Resolved, Failure> {
    let url = Url::parse(link).map_err(|error| {
        Failure::coded(
            FailureKind::Permanent,
            messages::INVALID_URL,
            messages::invalid_url(&error),
        )
        .with_param("error", error.to_string())
    })?;
    let link_host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !download_host_allowed(&link_host) {
        return Err(Failure::coded(
            FailureKind::Permanent,
            messages::DIRECT_LINK_FOREIGN,
            messages::direct_link_foreign(&link_host),
        )
        .with_param("host", link_host));
    }
    // One byte, with the referer the site expects, is enough to learn whether the link is
    // served and how long the file is.
    let probe = host
        .http(
            HttpRequest::get(url.to_string())
                .with_header("Range", "bytes=0-0")
                .with_header("Referer", referer()),
        )
        .await?;
    if matches!(probe.status, 403..=405) {
        return Err(Failure::coded(
            FailureKind::RateLimited(Some(REFUSED_LINK_RETRY_SECONDS)),
            messages::RATE_LIMITED,
            messages::rate_limited(probe.status),
        )
        .with_param("status", probe.status.to_string()));
    }
    ensure_http_status(&probe)?;
    Ok(Resolved {
        file_name: probe
            .header("content-disposition")
            .and_then(file_name_from_disposition)
            .or(page_name),
        size: probe
            .header("content-range")
            .and_then(page::content_range_total),
        url: probe.final_url,
        // The transfer must look like the browser session that earned it.
        headers: vec![Header::new("Referer", referer())],
        checksum: None,
    })
}

/// What `/json/<id>` says about a link: the file, `[]` for one that is gone, and - as
/// JDownloader reads it - a hash that is not the id asked for counts as gone too.
fn metadata_verdict(
    response: &HttpResponse,
    id: &str,
) -> (LinkStatus, Option<String>, Option<u64>) {
    #[derive(Deserialize)]
    struct Metadata {
        title: Option<String>,
        size: Option<String>,
        hash: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum MetadataAnswer {
        Missing(Vec<serde_json::Value>),
        File(Metadata),
    }
    if response.status == 404 {
        return (LinkStatus::Offline, None, None);
    }
    if !(200..=299).contains(&response.status) {
        return (LinkStatus::Unknown, None, None);
    }
    match serde_json::from_slice::<MetadataAnswer>(&response.body) {
        // The measured "gone" answer is exactly `[]`; a list with something in it is an
        // answer this code does not know, and is not called offline on a guess.
        Ok(MetadataAnswer::Missing(entries)) if entries.is_empty() => {
            (LinkStatus::Offline, None, None)
        }
        Ok(MetadataAnswer::Missing(_)) => (LinkStatus::Unknown, None, None),
        Ok(MetadataAnswer::File(file)) => match file.hash {
            Some(hash) if hash.eq_ignore_ascii_case(id) => (
                LinkStatus::Online,
                file.title.filter(|title| !title.is_empty()),
                file.size.as_deref().and_then(page::parse_size),
            ),
            Some(_) => (LinkStatus::Offline, None, None),
            None => (LinkStatus::Unknown, None, None),
        },
        Err(_) => (LinkStatus::Unknown, None, None),
    }
}

/// Whether a direct link's host lies inside the manifest's `download_domains`.
fn download_host_allowed(host: &str) -> bool {
    page::DOWNLOAD_HOSTS
        .iter()
        .any(|pattern| match pattern.strip_prefix("*.") {
            Some(suffix) => host
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.')),
            None => host == *pattern,
        })
}

/// The referer every transfer carries, as pyLoad sends it.
fn referer() -> String {
    format!("https://{}/", page::PRIMARY_DOMAIN)
}

/// A provider message as it may reach a log or the interface: one line, capped.
fn sanitised(message: &str) -> String {
    message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn file_name_from_disposition(value: &str) -> Option<String> {
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("filename")
            .then(|| value.trim_matches(['\'', '"']).to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    let kind = match response.status {
        200..=299 => return Ok(()),
        429 => FailureKind::RateLimited(None),
        500..=599 => FailureKind::Transient(None),
        _ => FailureKind::Permanent,
    };
    Err(Failure::coded(
        kind,
        messages::HTTP_ERROR,
        messages::http_error(response.status),
    )
    .with_param("status", response.status.to_string()))
}

fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

#[cfg(test)]
mod tests {
    use super::{download_host_allowed, matches, sanitised};

    #[test]
    fn download_hosts_follow_the_manifest_patterns() {
        for host in [
            "krakenfiles.com",
            "dl.krakenfiles.com",
            "s3.krakenfiles.com",
            "hs3.krakencloud.net",
        ] {
            assert!(download_host_allowed(host), "{host}");
        }
        for host in [
            "krakencloud.net",
            "krakenfiles.com.evil.example",
            "notkrakenfiles.com",
            "example.com",
            "",
        ] {
            assert!(!download_host_allowed(host), "{host}");
        }
    }

    #[test]
    fn matching_follows_the_url_table() {
        assert!(matches("https://krakenfiles.com/view/DP3nGKJNsX/file.html"));
        assert!(matches(
            "https://www.krakenfiles.com/embed-video/DP3nGKJNsX"
        ));
        assert!(!matches("https://krakenfiles.com/view/DP3nGKJNsX"));
        assert!(!matches("not a url"));
    }

    #[test]
    fn provider_messages_are_one_line_and_capped() {
        assert_eq!(sanitised("  captcha\n  not   valid "), "captcha not valid");
        assert_eq!(sanitised(&"x".repeat(400)).len(), 160);
    }
}

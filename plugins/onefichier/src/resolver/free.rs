//! The account-less website flow: the link page carries a form, posting it yields the direct
//! link.
//!
//! No captcha and no countdown here — 1fichier gates free downloads by IP slot instead, which is
//! why every marker this reads maps to a hold-off rather than to something a retry could fix.

use plugin_common::{
    Failure, FailureKind, Header, HttpRequest, HttpResponse, PluginHost, Resolved,
};
use url::Url;

use super::{coded, invalid_url};
use crate::{
    messages,
    page::{self, DownloadForm, PageError, StatedFile},
};

/// Runs the account-less website flow and turns its result into a transfer.
pub(super) async fn resolve<H: PluginHost>(host: &H, url: &Url) -> Result<Resolved, Failure> {
    let content_url = page::content_url(url)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?;
    let referer = referer_for(url);
    let (transfer, stated) = free_transfer(host, &content_url, &referer).await?;
    // The link page states both; the transfer's own `Content-Disposition` is the fallback, and
    // an error page carries none at all -- which is how `download.bin` got its name (RD-109-36).
    let file_name = transfer
        .header("content-disposition")
        .and_then(file_name_from_disposition)
        .or_else(|| stated.as_ref().map(|stated| stated.name.clone()));
    Ok(Resolved {
        url: transfer.final_url,
        file_name,
        // Carried so the transfer can contradict what it receives against what the hoster
        // announced, rather than booking any number of bytes as the file.
        size: stated.map(|stated| stated.size),
        // The transfer must look like the browser session that earned the link; 1fichier
        // rejects a direct link fetched without the page it came from as referer.
        headers: vec![Header::new("Referer", referer)],
        // 1fichier only exposes a Whirlpool checksum, which the contract does not carry.
        checksum: None,
    })
}

/// The page -> form -> direct-link sequence, up to and including the link's range probe, with
/// what the link page stated about the file.
async fn free_transfer<H: PluginHost>(
    host: &H,
    content_url: &str,
    referer: &str,
) -> Result<(HttpResponse, Option<StatedFile>), Failure> {
    let page_response = free_get(host, content_url, referer).await?;
    // A hotlink is possible: 1fichier serves some files straight from the link page, in which
    // case another user is paying for the traffic and no form is involved at all.
    if !is_html(&page_response) {
        free_status_failure(page_response.status)?;
        return Ok((page_response, None));
    }
    let body = page_response.text().into_owned();
    page_failure(&body, page_response.status)?;
    let stated = page::stated_file(&body);
    let Some(form) = page::download_form(&body) else {
        return Err(no_free_form(&body));
    };
    if page::is_password_protected(&form) {
        return Err(coded(FailureKind::Permanent, messages::PASSWORD_REQUIRED));
    }
    let action = form_action(&page_response.final_url, &form)?;
    let posted = free_post(host, &action, &page::free_form(&form), referer).await?;
    if !is_html(&posted) {
        free_status_failure(posted.status)?;
        return Ok((posted, stated));
    }
    let posted_body = posted.text().into_owned();
    page_failure(&posted_body, posted.status)?;
    let Some(link) = page::direct_link(&posted_body) else {
        return Err(no_free_link(&posted_body));
    };
    let transfer = free_get(host, &link, referer).await?;
    free_status_failure(transfer.status)?;
    Ok((transfer, stated))
}

/// A range-limited GET: enough of the answer to tell a file from a page without pulling the
/// whole file into the plugin's response budget.
async fn free_get<H: PluginHost>(
    host: &H,
    url: &str,
    referer: &str,
) -> Result<HttpResponse, Failure> {
    host.http(
        HttpRequest::get(url.to_owned())
            .with_header("Referer", referer.to_owned())
            .with_header("Range", "bytes=0-0"),
    )
    .await
}

async fn free_post<H: PluginHost>(
    host: &H,
    url: &str,
    fields: &[(String, String)],
    referer: &str,
) -> Result<HttpResponse, Failure> {
    host.http(
        HttpRequest::post(url.to_owned(), page::encode_form(fields))
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("Referer", referer.to_owned())
            .with_header("Range", "bytes=0-0"),
    )
    .await
}

/// Where the download form posts to: its own `action` when it names one, else the page itself.
fn form_action(page_url: &str, form: &DownloadForm) -> Result<String, Failure> {
    let base = Url::parse(page_url).map_err(|error| invalid_url(&error))?;
    match form.action.as_deref() {
        Some(action) => base
            .join(action)
            .map(|url| url.to_string())
            .map_err(|error| invalid_url(&error)),
        None => Ok(page_url.to_owned()),
    }
}

/// `https://<the uploader's own domain>/` — the referer 1fichier expects, and the domain JD
/// warns must never be normalised away.
fn referer_for(url: &Url) -> String {
    match url.host_str() {
        Some(host) => format!("https://{host}/"),
        None => "https://1fichier.com/".to_owned(),
    }
}

/// Reports what the page says instead of handing over a download. Every wait or limit marker
/// becomes an `IpBlocked`, so the scheduler holds back the hoster rather than this one link.
fn page_failure(html: &str, status: u16) -> Result<(), Failure> {
    if let Some(error) = page::page_error(html) {
        return Err(match error {
            PageError::Offline => coded(FailureKind::Offline, messages::FILE_OFFLINE),
            PageError::AccountRequired => {
                coded(FailureKind::AuthRequired, messages::ACCOUNT_REQUIRED)
            }
            PageError::NoFreeSlots => Failure::coded(
                FailureKind::IpBlocked(Some(300)),
                messages::NO_FREE_SLOTS.0,
                messages::NO_FREE_SLOTS.1,
            )
            .with_param("wait_seconds", "300"),
            PageError::IpBlocked(seconds) => Failure::coded(
                FailureKind::IpBlocked(Some(seconds)),
                messages::FREE_LIMIT_REACHED,
                messages::free_limit_reached(seconds),
            )
            .with_param("wait_seconds", seconds.to_string()),
            PageError::ServerError(seconds) => Failure::coded(
                FailureKind::Transient(Some(seconds)),
                messages::SERVER_ERROR.0,
                messages::SERVER_ERROR.1,
            ),
            PageError::TooFast => Failure::coded(
                FailureKind::RateLimited(Some(30)),
                messages::FLOOD.0,
                messages::FLOOD.1,
            ),
        });
    }
    // The page explained nothing; a non-2xx status still has to be reported rather than parsed
    // for a form that cannot be there.
    free_status_failure(status)
}

/// Maps a status the page body itself did not explain. Deliberately not [`crate::api`]'s
/// mapping: that one reads 401/403 as a bad API key, which the account-less flow never sends.
fn free_status_failure(status: u16) -> Result<(), Failure> {
    match status {
        200..=299 => Ok(()),
        404 | 410 => Err(coded(FailureKind::Offline, messages::FILE_OFFLINE)),
        429 => Err(Failure::coded(
            FailureKind::RateLimited(Some(300)),
            messages::FLOOD.0,
            messages::FLOOD.1,
        )),
        500..=599 => Err(coded(FailureKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(Failure::coded(
            FailureKind::Permanent,
            messages::HTTP_ERROR,
            messages::http_error(other),
        )
        .with_param("status", other.to_string())),
    }
}

fn no_free_form(html: &str) -> Failure {
    let diagnosis = page::diagnose(html);
    Failure::coded(
        FailureKind::Permanent,
        messages::NO_FREE_FORM,
        messages::no_free_form(&diagnosis),
    )
    .with_param("diagnosis", diagnosis)
}

fn no_free_link(html: &str) -> Failure {
    let diagnosis = page::diagnose(html);
    Failure::coded(
        FailureKind::Permanent,
        messages::NO_FREE_LINK,
        messages::no_free_link(&diagnosis),
    )
    .with_param("diagnosis", diagnosis)
}

fn is_html(response: &HttpResponse) -> bool {
    response
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"))
}

fn file_name_from_disposition(value: &str) -> Option<String> {
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("filename")
            .then(|| value.trim_matches(['\'', '"']).to_owned())
            .filter(|value| !value.is_empty())
    })
}

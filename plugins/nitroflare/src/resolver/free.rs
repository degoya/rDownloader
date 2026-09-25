//! The account-less website flow: start the server-side countdown, solve its captcha, wait it
//! out, then ask for the link.
//!
//! Unlike the XFileSharing hosters this is not a form flow at all — it is three calls to one
//! AJAX endpoint, which is why the headers below matter as much as the fields.

use plugin_common::{
    CaptchaChallenge, Failure, FailureKind, Header, HttpRequest, HttpResponse, PluginHost,
    Resolved, WidgetChallenge,
};
use url::Url;

use super::{coded, ensure_http_status, invalid_url};
use crate::{messages, page};

/// The main domain JD's `getPluginDomains()` lists first, and the only one this plugin's
/// manifest claims.
const PRIMARY_DOMAIN: &str = "nitroflare.com";

/// The endpoint both free-download steps post to.
const FREE_DOWNLOAD_PATH: &str = "/ajax/freeDownload.php";

/// Runs the account-less website flow and turns its result into a transfer.
pub(super) async fn resolve<H: PluginHost>(host: &H, file_id: &str) -> Result<Resolved, Failure> {
    let transfer = free_transfer(host, file_id).await?;
    let disposition = transfer.header("content-disposition").map(str::to_owned);
    if disposition.is_none() && is_html(&transfer) {
        return Err(no_free_link(&transfer.text()));
    }
    let final_url = Url::parse(&transfer.final_url).map_err(|error| invalid_url(&error))?;
    Ok(Resolved {
        url: transfer.final_url.clone(),
        file_name: disposition
            .as_deref()
            .and_then(page::file_name_from_disposition)
            .or_else(|| page::url_file_name(&final_url)),
        size: None,
        // The transfer must look like the browser session that earned the link; Nitroflare
        // refuses a download URL fetched without the page it came from as referer.
        headers: vec![Header::new("Referer", format!("https://{PRIMARY_DOMAIN}/"))],
        checksum: None,
    })
}

/// The whole flow, up to and including the direct link's range probe.
async fn free_transfer<H: PluginHost>(host: &H, file_id: &str) -> Result<HttpResponse, Failure> {
    let file_page = get(
        host,
        &format!("https://{PRIMARY_DOMAIN}/view/{file_id}"),
        None,
    )
    .await?;
    let body = file_page.text().into_owned();
    page_failure(&body)?;
    let Some(site_key) = page::recaptcha_site_key(&body) else {
        let diagnosis = page::diagnose(&body);
        return Err(Failure::coded(
            FailureKind::Permanent,
            messages::NO_FREE_MARKERS,
            messages::no_free_markers(&diagnosis),
        )
        .with_param("diagnosis", diagnosis));
    };
    let page_url = Url::parse(&file_page.final_url).map_err(|error| invalid_url(&error))?;
    let ajax_url = page_url
        .join(FREE_DOWNLOAD_PATH)
        .map_err(|error| invalid_url(&error))?
        .to_string();

    let stated = start_timer(host, &ajax_url, file_id, page_url.as_str()).await?;
    // JD reads the countdown from the file page and falls back to 60 seconds; a countdown stated
    // by the timer answer itself wins, since it is the more specific signal.
    let wait_seconds = stated
        .or_else(|| page::countdown_seconds(&body))
        .unwrap_or(page::DEFAULT_WAIT_SECONDS);

    // Solve first, wait second: the countdown runs server-side either way, and a token minted
    // before the wait would be closer to expiry when the form is finally posted.
    let token = host
        .solve_captcha(challenge(&site_key, page_url.as_str()))
        .await?
        .token;
    host.wait(clamp_seconds(wait_seconds)).await?;

    let answer = fetch_download(host, &ajax_url, &token, page_url.as_str()).await?;
    let Some(link) = page::direct_link(&answer) else {
        return Err(no_free_link(&answer));
    };
    let parsed = Url::parse(&link).map_err(|error| invalid_url(&error))?;
    if !parsed.host_str().is_some_and(page::is_provider_host) {
        let host_name = parsed.host_str().unwrap_or_default().to_owned();
        return Err(Failure::coded(
            FailureKind::Permanent,
            messages::FREE_LINK_HOST_MISMATCH,
            messages::free_link_host_mismatch(&host_name),
        )
        .with_param("host", host_name));
    }
    range_probe(host, &link, page_url.as_str()).await
}

/// Step 2: starts the server-side countdown. Returns the wait the answer stated, if any.
async fn start_timer<H: PluginHost>(
    host: &H,
    ajax_url: &str,
    file_id: &str,
    page_url: &str,
) -> Result<Option<u64>, Failure> {
    let response = ajax_post(
        host,
        ajax_url,
        &[("method", "startTimer"), ("fileId", file_id)],
        page_url,
    )
    .await?;
    let body = response.text();
    // The answer can carry a limit notice instead of the expected `1`; report that first, so a
    // blocked IP never looks like a broken parser.
    page_failure(&body)?;
    match page::timer_start(&body) {
        page::TimerStart::Started => Ok(None),
        page::TimerStart::Countdown(seconds) => Ok(Some(seconds)),
        page::TimerStart::Unrecognized => {
            let answer = page::diagnose(&body);
            Err(Failure::coded(
                FailureKind::Transient(Some(60)),
                messages::TIMER_NOT_STARTED,
                messages::timer_not_started(&answer),
            )
            .with_param("answer", answer))
        }
    }
}

/// Step 4: submits the solved captcha and returns the HTML fragment carrying the link.
async fn fetch_download<H: PluginHost>(
    host: &H,
    ajax_url: &str,
    token: &str,
    page_url: &str,
) -> Result<String, Failure> {
    let response = ajax_post(
        host,
        ajax_url,
        &[
            ("method", "fetchDownload"),
            // JD sends the token under both names; the site has used each of them.
            ("captcha", token),
            ("g-recaptcha-response", token),
        ],
        page_url,
    )
    .await?;
    let body = response.text().into_owned();
    page_failure(&body)?;
    if page::is_wrong_captcha(&body) {
        return Err(coded(
            FailureKind::CaptchaFailed,
            messages::CAPTCHA_REJECTED,
        ));
    }
    Ok(body)
}

async fn get<H: PluginHost>(
    host: &H,
    url: &str,
    referer: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let mut request = HttpRequest::get(url.to_owned());
    if let Some(referer) = referer {
        request = request.with_header("Referer", referer.to_owned());
    }
    send(host, request).await
}

async fn range_probe<H: PluginHost>(
    host: &H,
    url: &str,
    referer: &str,
) -> Result<HttpResponse, Failure> {
    send(
        host,
        HttpRequest::get(url.to_owned())
            .with_header("Range", "bytes=0-0")
            .with_header("Referer", referer.to_owned()),
    )
    .await
}

/// JD's `setAjaxHeaders` plus the `Referer`/`Origin` pair the endpoint's CSRF check wants.
async fn ajax_post<H: PluginHost>(
    host: &H,
    url: &str,
    fields: &[(&str, &str)],
    page_url: &str,
) -> Result<HttpResponse, Failure> {
    send(
        host,
        HttpRequest::post(url.to_owned(), page::encode_form(fields))
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("X-Requested-With", "XMLHttpRequest")
            .with_header("Accept", "*/*")
            .with_header("Referer", page_url.to_owned())
            .with_header("Origin", format!("https://{PRIMARY_DOMAIN}")),
    )
    .await
}

async fn send<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let response = host.http(request).await?;
    ensure_http_status(&response)?;
    Ok(response)
}

/// Aborts a free flow when the page reports the file is premium-only or this IP is limited.
fn page_failure(html: &str) -> Result<(), Failure> {
    if page::is_premium_only(html) {
        return Err(coded(FailureKind::AuthRequired, messages::PREMIUM_REQUIRED));
    }
    let Some(seconds) = page::ip_block_seconds(html) else {
        return Ok(());
    };
    // `Some(0)` means the page stated a limit without naming a duration; leave the delay to the
    // scheduler's own hold-off rather than inventing one here.
    let retry_after_seconds = (seconds > 0).then_some(seconds);
    let mut failure = Failure::coded(
        FailureKind::IpBlocked(retry_after_seconds),
        messages::FREE_LIMIT_REACHED,
        messages::free_limit_reached(retry_after_seconds),
    );
    if let Some(seconds) = retry_after_seconds {
        failure = failure.with_param("wait_seconds", seconds.to_string());
    }
    Err(failure)
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

fn challenge(site_key: &str, page_url: &str) -> CaptchaChallenge {
    CaptchaChallenge::RecaptchaV2(WidgetChallenge {
        site_key: site_key.to_owned(),
        page_url: page_url.to_owned(),
        invisible: false,
    })
}

fn is_html(response: &HttpResponse) -> bool {
    response
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"))
}

fn clamp_seconds(seconds: u64) -> u32 {
    u32::try_from(seconds).unwrap_or(u32::MAX)
}

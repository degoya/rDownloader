//! The account-less website flow: start the server-side countdown, solve its captcha, wait it
//! out, ask the session for its verdict, then post the captcha form.
//!
//! The longest of the free flows here, and the only one that may have to solve a captcha twice:
//! the site key is only authoritatively known after the countdown, and a token minted against a
//! stale key would be rejected.

use plugin_common::{
    CaptchaChallenge, Failure, FailureKind, Header, HttpRequest, HttpResponse, PluginHost,
    Resolved, WidgetChallenge,
};
use url::Url;

use super::{coded, convert_failure, invalid_url};
use crate::{messages, page};

/// The main domain JD's `getPluginDomains()` lists first; the `rg.to`/`rapidgator.asia` aliases
/// are rewritten to it before any request is made, exactly as `getContentURL` does.
const PRIMARY_DOMAIN: &str = "rapidgator.net";

/// The header Rapidgator's countdown endpoints require; they answer HTML to anything else.
const XML_HTTP_REQUEST: (&str, &str) = ("X-Requested-With", "XMLHttpRequest");

/// Runs the account-less website flow and turns its result into a transfer.
pub(super) async fn resolve<H: PluginHost>(
    host: &H,
    url: &Url,
    file_id: &str,
) -> Result<Resolved, Failure> {
    let url_name = page::url_file_name(url);
    let transfer = free_transfer(host, file_id).await?;
    let disposition = transfer.header("content-disposition").map(str::to_owned);
    if disposition.is_none() && is_html(&transfer) {
        return Err(no_free_link(&transfer.text()));
    }
    Ok(Resolved {
        url: transfer.final_url,
        file_name: disposition
            .as_deref()
            .and_then(page::file_name_from_disposition)
            .or(url_name),
        size: None,
        // The transfer must look like the browser session that earned the link; Rapidgator
        // refuses a download URL fetched without the page it came from as referer.
        headers: vec![Header::new("Referer", format!("https://{PRIMARY_DOMAIN}/"))],
        checksum: None,
    })
}

/// The whole flow, up to and including the final link's range probe.
async fn free_transfer<H: PluginHost>(host: &H, file_id: &str) -> Result<HttpResponse, Failure> {
    // `Range: bytes=0-0` on the file page only: a hoster that hotlinks the file answers with the
    // file itself, and this keeps that answer to a single byte instead of the response-size cap.
    let file_page = range_probe(
        host,
        &format!("https://{PRIMARY_DOMAIN}/file/{file_id}"),
        None,
    )
    .await?;
    // A hotlink is possible: an already-authorised session is served the file straight away.
    if !is_html(&file_page) {
        return Ok(file_page);
    }
    let body = file_page.text().into_owned();
    ip_block_failure(&body)?;
    let Some(markers) = page::timer_markers(&body) else {
        let diagnosis = page::diagnose(&body);
        return Err(Failure::coded(
            FailureKind::Permanent,
            messages::NO_FREE_MARKERS,
            messages::no_free_markers(&diagnosis),
        )
        .with_param("diagnosis", diagnosis));
    };
    let page_url = Url::parse(&file_page.final_url).map_err(|error| invalid_url(&error))?;
    let site_key = page::recaptcha_site_key(&body)
        .unwrap_or_else(|| page::RECAPTCHA_SITE_KEY_FALLBACK.to_owned());

    let sid = start_timer(host, &markers, &page_url).await?;
    // Solve first, wait second: the countdown runs server-side either way, and a token minted
    // before the wait would be closer to expiry when the form is finally posted.
    let token = host
        .solve_captcha(challenge(&site_key, page_url.as_str()))
        .await?
        .token;
    host.wait(clamp_seconds(markers.wait_seconds)).await?;
    fetch_download_link(host, &sid, &page_url).await?;

    let answer = submit_captcha(host, &page_url, &site_key, token).await?;
    let Some(link) = page::final_download_link(&answer) else {
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
    range_probe(host, &link, Some(page_url.as_str())).await
}

/// Step 3: starts the server-side countdown and returns its session id.
async fn start_timer<H: PluginHost>(
    host: &H,
    markers: &page::TimerMarkers,
    page_url: &Url,
) -> Result<String, Failure> {
    // `startTimerUrl` is page-relative in every observed page; `join` handles an absolute one
    // just as well.
    let timer_url = page_url
        .join(&markers.start_timer_url)
        .map_err(|error| invalid_url(&error))?;
    let response = xhr_get(
        host,
        timer_url.as_str(),
        vec![Header::new("fid", markers.fid.to_string())],
        page_url.as_str(),
    )
    .await?;
    let state = page::timer_state(&response.body).unwrap_or_default();
    if !state.is_state("started") {
        return Err(not_started(&state.state_text()));
    }
    state
        .sid
        .as_ref()
        .and_then(page::json_text)
        .ok_or_else(|| not_started("missing sid"))
}

/// Step 5: asks the countdown session for its verdict once the wait has passed.
async fn fetch_download_link<H: PluginHost>(
    host: &H,
    sid: &str,
    page_url: &Url,
) -> Result<(), Failure> {
    let url = base_url(page_url, "/download/AjaxGetDownloadLink")?;
    let response = xhr_get(host, &url, vec![Header::new("sid", sid)], page_url.as_str()).await?;
    let state = page::timer_state(&response.body).unwrap_or_default();
    if state.is_state("done") {
        return Ok(());
    }
    let state = state.state_text();
    Err(timer_failure(
        messages::DOWNLOAD_LINK_NOT_READY,
        messages::download_link_not_ready(&state),
        &state,
    ))
}

/// Step 6: fetches `/download/captcha` and posts its form. Returns the page the final link is
/// then looked for in — the posted answer, or the captcha page itself when it asks for none.
async fn submit_captcha<H: PluginHost>(
    host: &H,
    page_url: &Url,
    site_key: &str,
    token: String,
) -> Result<String, Failure> {
    let captcha_url = base_url(page_url, "/download/captcha")?;
    let captcha_page = get(host, &captcha_url, Some(page_url.as_str())).await?;
    let body = captcha_page.text().into_owned();
    ip_block_failure(&body)?;
    // JD: "Failed to find captchaform -> No captcha needed?" — the page is used as it is.
    let Some(form) = page::captcha_form(&body) else {
        return Ok(body);
    };
    let referer = captcha_page.final_url.clone();
    // The site key is only authoritatively known here, after the countdown; if it drifted from
    // the one the pre-wait solve used, the token would be rejected, so solve again.
    let token = match page::recaptcha_site_key(&body) {
        Some(key) if key != site_key => host.solve_captcha(challenge(&key, &referer)).await?.token,
        _ => token,
    };
    let action = match form.action.as_deref() {
        Some(action) => Url::parse(&referer)
            .and_then(|base| base.join(action))
            .map_err(|error| invalid_url(&error))?
            .to_string(),
        None => referer.clone(),
    };
    let posted = post_form(
        host,
        &action,
        &page::with_captcha_token(&form.fields, &token),
        &referer,
    )
    .await?;
    let posted_body = posted.text().into_owned();
    ip_block_failure(&posted_body)?;
    if page::is_wrong_captcha(&posted_body) {
        return Err(coded(
            FailureKind::CaptchaFailed,
            messages::CAPTCHA_REJECTED,
        ));
    }
    Ok(posted_body)
}

/// A plain page fetch. Unlike [`range_probe`] it sends no `Range`, because the pages after the
/// first one are always HTML and must arrive whole to be parsed.
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
    referer: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let mut request = HttpRequest::get(url.to_owned()).with_header("Range", "bytes=0-0");
    if let Some(referer) = referer {
        request = request.with_header("Referer", referer.to_owned());
    }
    send(host, request).await
}

async fn xhr_get<H: PluginHost>(
    host: &H,
    url: &str,
    query: Vec<Header>,
    referer: &str,
) -> Result<HttpResponse, Failure> {
    let mut request = HttpRequest::get(url.to_owned())
        .with_header(XML_HTTP_REQUEST.0, XML_HTTP_REQUEST.1)
        .with_header("Accept", "application/json, text/javascript, */*; q=0.01")
        .with_header("Referer", referer.to_owned());
    request.query = query;
    send(host, request).await
}

async fn post_form<H: PluginHost>(
    host: &H,
    url: &str,
    fields: &[(String, String)],
    referer: &str,
) -> Result<HttpResponse, Failure> {
    send(
        host,
        HttpRequest::post(url.to_owned(), page::encode_form(fields))
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("Referer", referer.to_owned())
            .with_header("Origin", format!("https://{PRIMARY_DOMAIN}")),
    )
    .await
}

async fn send<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let response = host.http(request).await?;
    // A 404 *is* trusted here, unlike on the `file/download` API endpoint: JD's
    // `checkOfflineWebsite` runs before every other website error check and maps a 404 response
    // code straight to `ERROR_FILE_NOT_FOUND`.
    crate::api::ensure_http_status(response.status, true).map_err(convert_failure)?;
    Ok(response)
}

/// Aborts a free flow when the page reports a free-download or IP limit.
fn ip_block_failure(html: &str) -> Result<(), Failure> {
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

/// Both countdown steps report a transient failure: the session is gone, but the link itself is
/// fine and a fresh attempt starts a new countdown.
fn timer_failure(code: &str, message: String, state: &str) -> Failure {
    Failure::coded(FailureKind::Transient(Some(60)), code, message).with_param("state", state)
}

fn not_started(state: &str) -> Failure {
    timer_failure(
        messages::TIMER_NOT_STARTED,
        messages::timer_not_started(state),
        state,
    )
}

fn challenge(site_key: &str, page_url: &str) -> CaptchaChallenge {
    CaptchaChallenge::RecaptchaV2(WidgetChallenge {
        site_key: site_key.to_owned(),
        page_url: page_url.to_owned(),
        invisible: false,
    })
}

/// `path` on the page's own origin, so the flow follows a redirect to a mirror host rather than
/// pinning every later request back to [`PRIMARY_DOMAIN`].
fn base_url(page_url: &Url, path: &str) -> Result<String, Failure> {
    page_url
        .join(path)
        .map(|url| url.to_string())
        .map_err(|error| invalid_url(&error))
}

fn is_html(response: &HttpResponse) -> bool {
    response
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"))
}

fn clamp_seconds(seconds: u64) -> u32 {
    u32::try_from(seconds).unwrap_or(u32::MAX)
}

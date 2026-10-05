//! The account-less (free) XFS download flow.
//!
//! Mirrors JD's `XFileSharingProBasic.doFree`, which `DdownloadCom.doFree` calls straight
//! through to (it only adds the "free dialog" prompt on top): fetch the file page, post the
//! `download1` form in free mode, solve the captcha the answer asks for, wait out the countdown,
//! post `download2`, and take the direct link from what comes back. Since 2026-09-17 the file
//! page carries the `download2` form directly and no `download1` at all (RD-108-28), so the
//! first step is skipped when the page already offers the second; an installation that still
//! serves the two-step form is handled as before. Every page is checked for an IP limit first,
//! because hitting one means no amount of waiting or captcha solving will help until it
//! expires. See `crate::page`'s module doc for the full IMPL-VERIFY record behind the `op`
//! values, the free button label, the `dk2CountdownNum` countdown and the `adblock_detected`
//! field.

use plugin_common::{
    Failure, FailureKind, HttpResponse, PluginHost, ResolveInput, Resolved,
    file_name_from_disposition,
};
use url::Url;

use super::api::{FREE, coded, ensure_http_status, invalid_url, is_html, range_probe};
use crate::{messages, page};

/// Runs the account-less XFS free flow and turns its result into a transfer.
pub(super) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
    parsed: &Url,
    code: &str,
) -> Result<Resolved, Failure> {
    let url_name = super::second_path_segment(parsed);
    let transfer = free_transfer(host, &request.url, code, url_name.as_deref()).await?;
    let disposition = transfer.header("content-disposition").map(str::to_owned);
    if disposition.is_none() && is_html(&transfer) {
        return Err(FREE.no_free_link(&transfer.text()));
    }
    Ok(Resolved {
        url: transfer.final_url,
        file_name: disposition
            .as_deref()
            .and_then(file_name_from_disposition)
            .or(url_name),
        size: None,
        // The transfer must look like the browser session that earned it; the hoster rejects a
        // direct link fetched without the page it came from as referer.
        headers: vec![super::referer_header()],
        checksum: None,
    })
}

/// The two-form free flow, up to and including the direct link's range probe.
async fn free_transfer<H: PluginHost>(
    host: &H,
    url: &str,
    code: &str,
    url_name: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let page_response = host.http(range_probe(url)).await?;
    ensure_http_status(&page_response)?;
    // A hotlink is possible: some XFS installations serve the file straight away.
    if !is_html(&page_response) {
        return Ok(page_response);
    }
    let body = page_response.text().into_owned();
    FREE.free_page_failure(&body)?;
    let (posted, posted_body) = match page::download1_form(&body) {
        Some(step_one) => {
            let posted = FREE
                .post_form(host, &page_response.final_url, &free_fields(&step_one))
                .await?;
            if !is_html(&posted) {
                return Ok(posted);
            }
            let posted_body = posted.text().into_owned();
            FREE.free_page_failure(&posted_body)?;
            (posted, posted_body)
        }
        // The file page measured on 2026-09-17 carries no `download1` form any more: the
        // `download2` form, its Turnstile widget and the countdown are on the page itself, so
        // the flow starts at the second step. Only a page offering neither form is not the
        // file page this plugin knows.
        None if page::download_form(&body).is_some() => (page_response, body),
        None => return Err(FREE.no_free_form(&body)),
    };
    let final_page = submit_download2(host, &posted, &posted_body).await?;
    if !is_html(&final_page) {
        return Ok(final_page);
    }
    let final_body = final_page.text().into_owned();
    FREE.free_page_failure(&final_body)?;
    let hints: Vec<&str> = url_name.into_iter().chain([code]).collect();
    // `page::direct_link` only accepts links on ddownload.com itself; a free link served from
    // one of the CDN aliases JD lists (`ucdn.to`, the `*.zeuscdn.org` hosts this plugin's
    // manifest already allows as download domains) would not be recognised and would surface
    // here as `no_free_link` rather than as a wrong download.
    let Some(link) = page::direct_link(&final_body, &hints) else {
        return Err(FREE.no_free_link(&final_body));
    };
    Url::parse(&link).map_err(|error| invalid_url(&error))?;
    let transfer = host.http(range_probe(link)).await?;
    ensure_http_status(&transfer)?;
    Ok(transfer)
}

/// Solves the captcha, waits out the countdown and posts `download2`. A rejected captcha is
/// retried once with a fresh challenge, the way JD's `download2` loop retries it.
async fn submit_download2<H: PluginHost>(
    host: &H,
    posted: &HttpResponse,
    posted_body: &str,
) -> Result<HttpResponse, Failure> {
    let Some(step_two) =
        page::download1_form(posted_body).or_else(|| page::download_form(posted_body))
    else {
        return Err(FREE.no_free_form(posted_body));
    };
    let mut attempt_body = posted_body.to_owned();
    let mut fields = step_two;
    for attempt in 0..2 {
        let mut submitted = free_fields(&fields);
        if let Some(marker) = page::widget_marker(&attempt_body) {
            let solution = host
                .solve_captcha(xfs_common::free::challenge_for(&marker, &posted.final_url))
                .await?;
            submitted = page::with_captcha_token(&submitted, marker.kind, &solution.token);
        }
        // JD solves the captcha first and then waits out the remainder, so the token is as
        // fresh as possible when the form is finally posted.
        if let Some(seconds) = page::free_wait_seconds(&attempt_body)
            && let Ok(seconds) = u32::try_from(seconds)
        {
            host.wait(seconds).await?;
        }
        let response = FREE.post_form(host, &posted.final_url, &submitted).await?;
        if !is_html(&response) {
            return Ok(response);
        }
        let body = response.text().into_owned();
        FREE.free_page_failure(&body)?;
        if !page::is_wrong_captcha(&body) {
            return Ok(response);
        }
        if attempt == 1 {
            break;
        }
        // Retry with whatever the rejection page now asks for.
        fields = page::download1_form(&body)
            .or_else(|| page::download_form(&body))
            .ok_or_else(|| FREE.no_free_form(&body))?;
        attempt_body = body;
    }
    Err(coded(
        FailureKind::CaptchaFailed,
        messages::CAPTCHA_REJECTED,
    ))
}

/// The free submission for either step: `method_free` kept, `method_premium` dropped, and
/// ddownload's `adblock_detected` field cleared when the form carries it.
fn free_fields(fields: &[(String, String)]) -> Vec<(String, String)> {
    page::with_adblock_cleared(&page::free_form(fields))
}

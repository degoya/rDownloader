//! The account-less (free) XFS download flow.
//!
//! The standard shape, and only the standard shape: fetch the file page, post the `download1`
//! form in free mode, solve whatever captcha the answer asks for, wait out the countdown, post
//! `download2`, and take the direct link from what comes back. Every page is checked for an IP
//! limit first, because hitting one means no amount of waiting or captcha solving will help
//! until it expires.
//!
//! This mirrors `plugins/filejoker/src/resolver/free.rs` with the site-specific markers removed.
//! Where that plugin recognises FileJoker's own phrasings for an offline file, a premium-only
//! file and a size limit, this one does not: it cannot know which clone it is talking to, and
//! guessing would produce a confident wrong answer. Such a page falls through to `no_free_form`
//! or `no_free_link` (`xfs_common::free::FreeWords`), both of which carry the page's own
//! diagnosis, so the failure names a cause instead of being empty.

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
    let transfer = free_transfer(host, &request.url, parsed, code, url_name.as_deref()).await?;
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
        // The transfer must look like the browser session that earned it; XFS installations
        // reject a direct link fetched without the page it came from as referer.
        headers: vec![super::referer_header(parsed)],
        checksum: None,
    })
}

/// The two-form free flow, up to and including the direct link's range probe.
async fn free_transfer<H: PluginHost>(
    host: &H,
    url: &str,
    parsed: &Url,
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
    let Some(step_one) = page::download1_form(&body) else {
        return Err(FREE.no_free_form(&body));
    };
    let posted = FREE
        .post_form(host, &page_response.final_url, &page::free_form(&step_one))
        .await?;
    if !is_html(&posted) {
        return Ok(posted);
    }
    let posted_body = posted.text().into_owned();
    FREE.free_page_failure(&posted_body)?;
    let final_page = submit_download2(host, &posted, &posted_body).await?;
    if !is_html(&final_page) {
        return Ok(final_page);
    }
    let final_body = final_page.text().into_owned();
    FREE.free_page_failure(&final_body)?;
    let Some(link) = direct_link(&final_body, parsed, code, url_name) else {
        return Err(FREE.no_free_link(&final_body));
    };
    let parsed_link = Url::parse(&link).map_err(|error| invalid_url(&error))?;
    // The link may sit on a delivery host of the same site rather than on the page's own host;
    // what it must not do is leave the sandbox, and the host's network gate enforces that on the
    // request below whatever this plugin believes.
    drop(parsed_link);
    let transfer = host.http(range_probe(link)).await?;
    ensure_http_status(&transfer)?;
    Ok(transfer)
}

/// The direct link on the final page.
///
/// Candidate hosts are the page's own host and its registrable-looking parent, so a delivery
/// subdomain of the same site is accepted while an unrelated host is not.
fn direct_link(html: &str, parsed: &Url, code: &str, url_name: Option<&str>) -> Option<String> {
    let host = parsed.host_str()?;
    let hints: Vec<&str> = url_name.into_iter().chain([code]).collect();
    let mut domains = vec![host];
    if let Some((_, parent)) = host.split_once('.')
        && parent.contains('.')
    {
        domains.push(parent);
    }
    page::direct_link(html, &hints, &domains)
}

/// Solves the captcha, waits out the countdown and posts `download2`. A rejected captcha is
/// retried once with a fresh challenge, the way the XFS flow's own retry loop does.
async fn submit_download2<H: PluginHost>(
    host: &H,
    posted: &HttpResponse,
    posted_body: &str,
) -> Result<HttpResponse, Failure> {
    let Some(step_two) =
        page::download1_form(posted_body).or_else(|| page::download2_form(posted_body))
    else {
        return Err(FREE.no_free_form(posted_body));
    };
    let mut attempt_body = posted_body.to_owned();
    let mut fields = step_two;
    for attempt in 0..2 {
        let mut submitted = page::free_form(&fields);
        if let Some(marker) = page::widget_marker(&attempt_body) {
            let solution = host
                .solve_captcha(xfs_common::free::challenge_for(&marker, &posted.final_url))
                .await?;
            submitted = page::with_captcha_token(&submitted, marker.kind, &solution.token);
        }
        // The captcha is solved first and the remainder of the countdown waited out after, so
        // the token is as fresh as possible when the form is finally posted.
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
            .or_else(|| page::download2_form(&body))
            .ok_or_else(|| FREE.no_free_form(&body))?;
        attempt_body = body;
    }
    Err(coded(
        FailureKind::CaptchaFailed,
        messages::CAPTCHA_REJECTED,
    ))
}

#[cfg(test)]
#[path = "free/tests.rs"]
mod tests;

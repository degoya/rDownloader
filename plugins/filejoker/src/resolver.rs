//! FileJoker's protocol logic, written once for both builds.
//!
//! FileJoker runs the same XFileSharing engine as `plugins/ddownload`, without its JSON API: the
//! cookie session is the whole credential, so there is no metadata call, no direct-link endpoint
//! and no link check. Every answer comes from the pages themselves, which is why the failure
//! classification below is where this plugin's real logic lives.

pub(crate) mod api;
mod free;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse, Label, LinkCheck,
    PluginHost, ResolveInput, Resolved,
};
use url::Url;

use self::api::{
    PRIMARY_DOMAIN, coded, ensure_http_status, file_code, file_name_from_disposition, invalid_url,
    is_html, range_probe,
};
use crate::{messages, page};

/// The plugin's own display name, for the one log line an unrecognized session page leaves
/// behind. A constant, never a value off the wire.
const PROVIDER: &str = "FileJoker";

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(|url| xfs_common::api::file_code(url, api::MATCH_HOSTS))
        .is_some()
}

/// Hoster domains this account can download from. A single hoster serves its own, so neither
/// the host nor the account changes the answer — the arguments are here because a multihoster's
/// catalogue does depend on both.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(crate::HOSTERS
        .iter()
        .map(|host| (*host).to_owned())
        .collect())
}

/// FileJoker has no metadata API, so there is nothing to check a link against without fetching
/// its page — which is what resolving does anyway.
pub(crate) async fn check<H: PluginHost>(
    _host: &H,
    _request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    Err(coded(FailureKind::Unsupported, messages::CHECK_UNSUPPORTED))
}

/// The cookie session is the account: it is verified against the site rather than assumed.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    let cookies = require_cookies(host, account_id).await?;
    let response = host
        .http(HttpRequest::get(format!("https://{PRIMARY_DOMAIN}/")))
        .await?;
    ensure_http_status(&response)?;
    // Only a page offering the sign-out link is a session; the login form and the guest
    // homepage a lapsed session lands on are not. Anything else is neither, and says so:
    // FileJoker's own pages have not been measured at all, so this rests on the XFS engine's
    // `op=logout` link, and a rule resting on an unmeasured marker is exactly the one that
    // must not condemn an account it merely failed to recognize (RD-108-28 and its review).
    // The classification is the shared one all three XFS plugins run (RD-120-13).
    //
    // Still the homepage, deliberately (RD-120-46). DDownload moved to its account page
    // because that page's behaviour is measured there; for FileJoker no page has been
    // measured at all, so `/?op=my_account` would trade one unmeasured page for another. With
    // no API, the session is the only proof of the account, so a page that settles nothing is
    // neither a pass nor a verdict: it is reported as unconfirmed, retryable, and traced in the
    // log — title, length and markers, never the body — so the next one can be measured.
    let body = response.text();
    match page::classify_session(&body) {
        page::SessionState::Active => {}
        page::SessionState::Expired(diagnosis) => {
            return Err(Failure::coded(
                FailureKind::AccountInvalid,
                messages::SESSION_INVALID,
                messages::session_invalid(&diagnosis),
            )
            .with_param("diagnosis", diagnosis));
        }
        page::SessionState::Unconfirmed(diagnosis) => {
            host.log(
                "warn",
                &crate::session_trace::unconfirmed_page_line(PROVIDER, "the homepage", &body),
            );
            return Err(Failure::coded(
                FailureKind::Transient(None),
                messages::SESSION_UNCONFIRMED,
                messages::session_unconfirmed(&diagnosis),
            )
            .with_param("diagnosis", diagnosis));
        }
    }
    Ok(Account {
        valid: true,
        // The sign-out link proves the session, and nothing here proves the subscription:
        // FileJoker has no metadata API, so no branch of this plugin ever reads an expiry or a
        // plan. This answered `true` regardless until RD-109-38, so a free account with a
        // working cookie session was reported as "Premium active" exactly like a paid one.
        premium: false,
        // The count first, then what the request actually established. A count on its own is
        // what made a green check meaningless elsewhere, so it never stands alone here either.
        label: Label::new()
            .cookies(cookies.len())
            .session_active()
            .premium_unchecked()
            .into(),
        traffic_left: None,
    })
}

/// Turns a link into a download: the free flow without an account, the cookie-backed premium
/// flow with one.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url).map_err(|error| invalid_url(&error))?;
    let code = file_code(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?
        .to_owned();
    // The free flow runs without any credential at all, so the cookie gate is deliberately not
    // on its path.
    let Some(account_id) = request.account_id.as_deref() else {
        return free::resolve(host, request, &parsed, &code).await;
    };
    require_cookies(host, account_id).await?;
    let url_name = second_path_segment(&parsed);
    let transfer = premium_transfer(host, &request.url, &code, url_name.as_deref()).await?;
    let disposition = transfer.header("content-disposition").map(str::to_owned);
    if disposition.is_none() && is_html(&transfer) {
        return Err(page_failure(&transfer.text()));
    }
    Ok(Resolved {
        url: transfer.final_url,
        file_name: disposition
            .as_deref()
            .and_then(file_name_from_disposition)
            .or(url_name),
        size: None,
        headers: Vec::new(),
        checksum: None,
    })
}

/// Fails before issuing any request when the account has no cookie session at all.
async fn require_cookies<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Vec<(String, String)>, Failure> {
    let cookies = host
        .cookies(account_id, &format!("https://{PRIMARY_DOMAIN}/"))
        .await;
    if cookies.is_empty() {
        return Err(coded(FailureKind::AuthRequired, messages::COOKIES_MISSING));
    }
    Ok(cookies)
}

/// Runs the XFileSharing premium flow. Both fetched pages are checked for FileJoker's offline
/// and wait markers before the generic form parsing runs, and the found `download2` form is
/// checked for a captcha before it is posted — this plugin cannot solve one there.
async fn premium_transfer<H: PluginHost>(
    host: &H,
    url: &str,
    code: &str,
    url_name: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let page_response = host.http(range_probe(url)).await?;
    ensure_http_status(&page_response)?;
    if !is_html(&page_response) {
        return Ok(page_response);
    }
    let body = page_response.text().into_owned();
    if let Some(failure) = offline_or_wait_failure(&body) {
        return Err(failure);
    }
    let Some(form) = page::download_form(&body) else {
        return Err(page_failure(&body));
    };
    if page::form_html(&body).is_some_and(page::has_captcha_challenge) {
        return Err(coded(FailureKind::NeedsCaptcha, messages::CAPTCHA_REQUIRED));
    }
    let posted = host
        .http(
            HttpRequest::post(
                page_response.final_url.clone(),
                page::encode_form(&page::premium_form(&form)),
            )
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("Range", "bytes=0-0"),
        )
        .await?;
    ensure_http_status(&posted)?;
    if !is_html(&posted) {
        return Ok(posted);
    }
    let posted_body = posted.text().into_owned();
    if let Some(failure) = offline_or_wait_failure(&posted_body) {
        return Err(failure);
    }
    let hints: Vec<&str> = url_name.into_iter().chain([code]).collect();
    let Some(link) = page::direct_link(&posted_body, &hints) else {
        return Err(page_failure(&posted_body));
    };
    Url::parse(&link).map_err(|error| invalid_url(&error))?;
    let transfer = host.http(range_probe(link)).await?;
    ensure_http_status(&transfer)?;
    Ok(transfer)
}

/// FileJoker's file-not-found marker first, then its pre-download wait. `None` when neither is
/// present.
fn offline_or_wait_failure(html: &str) -> Option<Failure> {
    if page::is_file_offline(html) {
        return Some(coded(FailureKind::Offline, messages::FILE_OFFLINE));
    }
    let seconds = page::estimated_wait_seconds(html)?;
    Some(
        Failure::coded(
            FailureKind::Transient(Some(seconds)),
            messages::DOWNLOAD_WAIT,
            messages::download_wait(seconds),
        )
        .with_param("wait_seconds", seconds.to_string()),
    )
}

/// Classifies an HTML page that turned out not to be the file: a login wall, a premium-only
/// notice, or an unrecognised page shape, in that order.
pub(super) fn page_failure(html: &str) -> Failure {
    let diagnosis = page::diagnose(html);
    if page::is_login_wall(html) {
        return Failure::coded(
            FailureKind::AccountInvalid,
            messages::SESSION_INVALID,
            messages::session_invalid(&diagnosis),
        )
        .with_param("diagnosis", diagnosis);
    }
    if page::is_premium_only(html) {
        return Failure::coded(
            FailureKind::AccountInvalid,
            messages::NO_PREMIUM_FILE,
            messages::no_premium_file(&diagnosis),
        )
        .with_param("diagnosis", diagnosis);
    }
    Failure::coded(
        FailureKind::Permanent,
        messages::PAGE_ERROR,
        messages::page_error(&diagnosis),
    )
    .with_param("diagnosis", diagnosis)
}

/// The file name segment of a `/<code>/<name>` link.
fn second_path_segment(url: &Url) -> Option<String> {
    url.path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).nth(1))
        .map(str::to_owned)
}

/// The `Referer` a free transfer must carry, so the hoster sees the page that earned it.
pub(crate) fn referer_header() -> Header {
    Header::new("Referer", format!("https://{PRIMARY_DOMAIN}/"))
}

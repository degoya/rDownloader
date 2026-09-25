//! KatFile's protocol logic, written once for both builds.
//!
//! Every function here takes the host as a parameter rather than reaching for one, so the same
//! code runs against the native `ResolverHost` and against the WIT imports. The two adapters
//! that supply it — `native.rs` and `guest.rs` — hold nothing but type conversions.
//!
//! KatFile runs the same XFileSharing engine as `plugins/ddownload`, with three additions JD's
//! `KatfileCom` makes: every link is rewritten to the current main domain before it is browsed,
//! both fetched pages are checked for a premium-only marker and a pre-download wait, and the
//! `download2` form is checked for a captcha before it is posted — this plugin cannot solve one
//! there, so it says so instead of submitting a form the server will reject.
//!
//! Premium status is decided by comparing the account's expiry against the host's clock, and
//! only where that expiry exists. Both builds used to guess at it differently — the native one
//! with `Utc::now()`, the guest by asserting `premium: true` — because the guest had no clock
//! at all; `host.now-unix-seconds` removed the reason for the guess. The cookie-only branch has
//! no expiry to compare at any clock, so it reports `premium: false` and says in its label that
//! the subscription was not read (RD-109-38).

pub(crate) mod api;
mod free;

#[cfg(test)]
mod direct_link_tests;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse, Label, LabelPart,
    LinkCheck, LinkStatus, PluginHost, ResolveInput, Resolved,
};
use url::Url;

use xfs_common::api::DirectLinkSkip;

use self::api::{
    AccountResult, DirectLink, FileInfo, MATCH_HOSTS, api_request, coded, convert_envelope_error,
    ensure_http_status, file_code, file_name_from_disposition, invalid_url, is_html, parse_json,
    range_probe,
};
use crate::{messages, page};

/// The plugin's own display name, for the one log line a silent fallback leaves behind. A
/// constant, never a value off the wire: that is what keeps a key or an address out of a log.
const PROVIDER: &str = "KatFile";

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(|url| xfs_common::api::file_code(url, MATCH_HOSTS))
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

/// What the account is worth, through the API key when there is one and through the cookie
/// session otherwise.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    if !host
        .secret_available(account_id, api::API_KEY_REFERENCE)
        .await
    {
        // No API key: the session is the account, so it is verified rather than assumed.
        let scope = format!("https://{}/", api::PRIMARY_DOMAIN);
        let cookies = host.cookies(account_id, &scope).await;
        if cookies.is_empty() {
            return Err(coded(
                FailureKind::AuthRequired,
                messages::COOKIE_SESSION_REQUIRED,
            ));
        }
        let response = host.http(HttpRequest::get(scope)).await?;
        ensure_http_status(&response)?;
        // A 200 says the site answered, not that it knows the session: an expired cookie jar
        // is served the guest homepage with exactly that status, and until RD-120-13 this
        // branch asked for nothing more. Only the site's own sign-out link is a session, and
        // only the site's own guest markup disproves one — a page carrying neither is
        // unrecognized, not a broken account.
        //
        // Still the homepage, deliberately (RD-120-46). DDownload moved to its account page
        // because that page's behaviour is measured there; for KatFile no page has been
        // measured at all, signed in or not, so `/?op=my_account` would trade one unmeasured
        // page for another. And here nothing but the session proves the account, so a page
        // that settles nothing is neither a pass nor a verdict: it is reported as unconfirmed,
        // retryable, and traced in the log so the next one can be measured.
        let body = response.text();
        match page::classify_session(&body) {
            page::SessionState::Active => {}
            page::SessionState::Expired(diagnosis) => {
                return Err(Failure::coded(
                    FailureKind::AccountInvalid,
                    messages::COOKIE_SESSION_INVALID,
                    messages::cookie_session_invalid(&diagnosis),
                )
                .with_param("diagnosis", diagnosis));
            }
            page::SessionState::Unconfirmed(diagnosis) => {
                trace_unconfirmed(host, &body);
                return Err(Failure::coded(
                    FailureKind::Transient(None),
                    messages::COOKIE_SESSION_UNCONFIRMED,
                    messages::cookie_session_unconfirmed(&diagnosis),
                )
                .with_param("diagnosis", diagnosis));
            }
        }
        return Ok(Account {
            valid: true,
            // The verified session proves the cookies, and nothing here proves the
            // subscription: `premium_expire` lives behind `api/account/info`, which knows only
            // API keys, and this is the branch that has none. This answered `true` regardless
            // until RD-109-38, so a free account was reported as "Premium active" exactly like
            // a paid one.
            premium: false,
            label: Label::new()
                .cookies(cookies.len())
                .session_active()
                .premium_unchecked()
                .into(),
            traffic_left: None,
        });
    }
    let info = account_info(host).await?;
    let premium = premium_until(host.now_unix_seconds().await, &info.premium_expire);
    // The key proves the account; the download runs on the cookie session, and until RD-120-13
    // nothing here looked at that session at all — it counted the jar and reported the count.
    let label = match verify_download_session(host, account_id).await? {
        DownloadSession::Absent => crate::account::account_label(&info.email, 0, false),
        DownloadSession::Confirmed(cookies) => {
            crate::account::account_label(&info.email, cookies, true)
        }
        DownloadSession::Unconfirmed(cookies) => {
            crate::account::account_label(&info.email, cookies, false).part(LabelPart::coded(
                messages::SESSION_UNCONFIRMED.0,
                messages::SESSION_UNCONFIRMED.1,
            ))
        }
    };
    Ok(Account {
        valid: true,
        premium,
        label: label.into(),
        traffic_left: xfs_common::api::traffic_left_bytes(info.traffic_left),
    })
}

/// What the probe learned about the cookie session a download runs on.
enum DownloadSession {
    /// No cookies: nothing to ask the site about.
    Absent,
    /// The site showed the sign-out link to this many cookies.
    Confirmed(usize),
    /// The site answered with a page that settles nothing.
    Unconfirmed(usize),
}

/// Verifies the cookie session a download will run on.
///
/// The request the `api_key` branch of [`check_account`] used to leave out. The key answers for
/// the *account* and says nothing whatever about the browser session the premium flow needs, so
/// the check counted the cookies instead and reported the count as though it were a verdict
/// (RD-120-13). Here the account is proven and only the session can be gone, so the refusal
/// says that rather than condemning the account.
///
/// A page that settles nothing is no refusal either (RD-120-46, after DDownload's RD-120-44):
/// the key has proven the account, and not knowing about the session is no reason to fail a
/// check it passed. The label says the session is unconfirmed, and the page is traced in the
/// log. It asks the homepage, as the cookie-only branch does and for the reason given there.
///
/// Zero cookies is deliberately not a refusal: link checks run on the key alone, so an account
/// held for them is legitimate, and the label states the absence in words.
async fn verify_download_session<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<DownloadSession, Failure> {
    let scope = format!("https://{}/", api::PRIMARY_DOMAIN);
    let cookies = host.cookies(account_id, &scope).await;
    if cookies.is_empty() {
        return Ok(DownloadSession::Absent);
    }
    let response = host.http(HttpRequest::get(scope)).await?;
    ensure_http_status(&response)?;
    let body = response.text();
    match page::classify_session(&body) {
        page::SessionState::Active => Ok(DownloadSession::Confirmed(cookies.len())),
        page::SessionState::Expired(diagnosis) => Err(Failure::coded(
            FailureKind::AuthRequired,
            messages::DOWNLOAD_SESSION_EXPIRED,
            messages::download_session_expired(&diagnosis),
        )
        .with_param("diagnosis", diagnosis)),
        page::SessionState::Unconfirmed(_) => {
            trace_unconfirmed(host, &body);
            Ok(DownloadSession::Unconfirmed(cookies.len()))
        }
    }
}

/// The one log line a session page nobody recognizes leaves: title, length and markers, never
/// the body (RD-120-46).
fn trace_unconfirmed<H: PluginHost>(host: &H, body: &str) {
    host.log(
        "warn",
        &crate::session_trace::unconfirmed_page_line(PROVIDER, "the homepage", body),
    );
}

/// Turns a link into a download: the free flow without an account, otherwise the API's direct
/// link when it answers and the cookie-backed premium flow when it does not.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url)
        .map_err(|_| coded(FailureKind::Permanent, messages::INVALID_LINK))?;
    let code = file_code(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?
        .to_owned();
    let Some(account_id) = request.account_id.as_deref() else {
        return free::resolve(host, request, &parsed, &code).await;
    };
    let has_api_key = host
        .secret_available(account_id, api::API_KEY_REFERENCE)
        .await;
    if has_api_key && let Some(resolved) = direct_link(host, &code).await {
        return Ok(resolved);
    }
    if cookie_count(host, account_id).await == 0 {
        return Err(coded(
            FailureKind::AuthRequired,
            messages::COOKIE_SESSION_REQUIRED_FOR_DOWNLOAD,
        ));
    }
    let metadata = if has_api_key {
        file_info(host, &code).await?
    } else {
        None
    };
    let url_name = second_path_segment(&parsed);
    let transfer = premium_transfer(host, &request.url, &code, url_name.as_deref()).await?;
    let disposition = transfer.header("content-disposition").map(str::to_owned);
    if disposition.is_none() && is_html(&transfer) {
        let diagnosis = page::diagnose(&transfer.text());
        return Err(Failure::coded(
            FailureKind::AccountInvalid,
            messages::NO_PREMIUM_FILE,
            messages::no_premium_file(&diagnosis),
        )
        .with_param("diagnosis", diagnosis));
    }
    let file_name = metadata
        .as_ref()
        .and_then(|item| item.name.clone())
        .or_else(|| disposition.as_deref().and_then(file_name_from_disposition))
        .or(url_name);
    Ok(Resolved {
        url: transfer.final_url,
        file_name,
        size: metadata
            .and_then(|item| item.size)
            .and_then(xfs_common::api::FlexibleU64::into_u64),
        headers: Vec::new(),
        checksum: None,
    })
}

/// Batched link check through the metadata API, which is the only thing that can answer it.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let Some(account_id) = request.account_id.as_deref() else {
        return Err(coded(FailureKind::AuthRequired, messages::ACCOUNT_MISSING));
    };
    if !host
        .secret_available(account_id, api::API_KEY_REFERENCE)
        .await
    {
        return Err(coded(FailureKind::AuthRequired, messages::API_KEY_REQUIRED));
    }
    let coded: Vec<(String, Option<String>)> = request
        .urls
        .iter()
        .map(|url| {
            let code = Url::parse(url)
                .ok()
                .and_then(|parsed| file_code(&parsed).map(str::to_owned));
            (url.clone(), code)
        })
        .collect();
    let mut results = Vec::with_capacity(coded.len());
    for chunk in coded.chunks(50) {
        let codes: Vec<&str> = chunk
            .iter()
            .filter_map(|(_, code)| code.as_deref())
            .collect();
        let infos = if codes.is_empty() {
            Vec::new()
        } else {
            let response = host
                .http(api_request("file/info", &[("file_code", codes.join(","))]))
                .await?;
            ensure_http_status(&response)?;
            let envelope: xfs_common::api::ApiEnvelope<Vec<FileInfo>> = parse_json(&response)?;
            envelope.into_result().map_err(convert_envelope_error)?
        };
        let mut infos = infos.into_iter();
        for (url, code) in chunk {
            if code.is_none() {
                results.push(LinkCheck {
                    url: url.clone(),
                    status: LinkStatus::Unknown,
                    file_name: None,
                    size: None,
                });
                continue;
            }
            let info = infos.next();
            results.push(LinkCheck {
                url: url.clone(),
                status: match info.as_ref().map(|item| item.status) {
                    Some(200) => LinkStatus::Online,
                    Some(404) => LinkStatus::Offline,
                    _ => LinkStatus::Unknown,
                },
                file_name: info.as_ref().and_then(|item| item.name.clone()),
                size: info
                    .and_then(|item| item.size)
                    .and_then(xfs_common::api::FlexibleU64::into_u64),
            });
        }
    }
    Ok(results)
}

async fn account_info<H: PluginHost>(host: &H) -> Result<AccountResult, Failure> {
    let response = host.http(api_request("account/info", &[])).await?;
    ensure_http_status(&response)?;
    let envelope: xfs_common::api::ApiEnvelope<AccountResult> = parse_json(&response)?;
    envelope.into_result().map_err(convert_envelope_error)
}

async fn file_info<H: PluginHost>(host: &H, code: &str) -> Result<Option<FileInfo>, Failure> {
    let response = host
        .http(api_request("file/info", &[("file_code", code.to_owned())]))
        .await?;
    ensure_http_status(&response)?;
    let envelope: xfs_common::api::ApiEnvelope<Vec<FileInfo>> = parse_json(&response)?;
    let info = envelope
        .into_result()
        .map_err(convert_envelope_error)?
        .into_iter()
        .next();
    if info.as_ref().is_some_and(|item| item.status != 200) {
        return Err(coded(FailureKind::Permanent, messages::FILE_UNAVAILABLE));
    }
    Ok(info)
}

/// Some XFileSharing installations expose `file/direct_link` for premium API keys; KatFile does
/// not document it either, so any failure falls back to the cookie flow.
///
/// Quiet as far as the download is concerned, no longer silent: the reason goes out exactly
/// once per attempt, as one of [`DirectLinkSkip`]'s fixed phrases, so no file code, address or
/// key can travel in it (RD-120-13).
async fn direct_link<H: PluginHost>(host: &H, code: &str) -> Option<Resolved> {
    match direct_link_attempt(host, code).await {
        Ok(resolved) => Some(resolved),
        Err(skip) => {
            host.log(
                "info",
                &xfs_common::api::direct_link_skipped(PROVIDER, skip),
            );
            None
        }
    }
}

/// The attempt itself, with every way it can come to nothing named rather than swallowed.
async fn direct_link_attempt<H: PluginHost>(
    host: &H,
    code: &str,
) -> Result<Resolved, DirectLinkSkip> {
    let response = host
        .http(api_request(
            "file/direct_link",
            &[("file_code", code.to_owned())],
        ))
        .await
        .map_err(|_| DirectLinkSkip::RequestFailed)?;
    let envelope: xfs_common::api::ApiEnvelope<DirectLink> =
        parse_json(&response).map_err(|_| DirectLinkSkip::NotJson)?;
    let link = envelope
        .into_result()
        .map_err(|_| DirectLinkSkip::ApiError)?;
    let url = Url::parse(&link.url).map_err(|_| DirectLinkSkip::UnparsableUrl)?;
    if !url.host_str().is_some_and(|host| {
        host == api::PRIMARY_DOMAIN || host.ends_with(&format!(".{}", api::PRIMARY_DOMAIN))
    }) {
        return Err(DirectLinkSkip::ForeignHost);
    }
    Ok(Resolved {
        file_name: url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        size: link.size.and_then(xfs_common::api::FlexibleU64::into_u64),
        url: url.to_string(),
        headers: Vec::new(),
        checksum: None,
    })
}

async fn cookie_count<H: PluginHost>(host: &H, account_id: &str) -> usize {
    host.cookies(account_id, &format!("https://{}/", api::PRIMARY_DOMAIN))
        .await
        .len()
}

/// Runs the XFileSharing premium flow: the file page carries a `download2` form that must be
/// posted with the logged-in cookie session; the answer is either the file (via redirect) or a
/// page carrying the direct link.
async fn premium_transfer<H: PluginHost>(
    host: &H,
    url: &str,
    code: &str,
    url_name: Option<&str>,
) -> Result<HttpResponse, Failure> {
    let page_response = host.http(range_probe(api::canonicalize_host(url))).await?;
    ensure_http_status(&page_response)?;
    if !is_html(&page_response) {
        return Ok(page_response);
    }
    let body = page_response.text().into_owned();
    if let Some(failure) = premium_only_or_wait_failure(&body, &page_response.final_url) {
        return Err(failure);
    }
    let Some(form) = page::download_form(&body) else {
        return Ok(page_response);
    };
    // JD hands `handleCaptcha` the found form, not the page, so a widget elsewhere — a login
    // modal, a site banner — never aborts a resolve.
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
    if let Some(failure) = premium_only_or_wait_failure(&posted_body, &posted.final_url) {
        return Err(failure);
    }
    let hints: Vec<&str> = url_name.into_iter().chain([code]).collect();
    let Some(link) = page::direct_link(&posted_body, &hints) else {
        return Ok(posted);
    };
    Url::parse(&link).map_err(|error| invalid_url(&error))?;
    let transfer = host.http(range_probe(link)).await?;
    ensure_http_status(&transfer)?;
    Ok(transfer)
}

/// KatFile's premium-only markers first, then its pre-download wait, mirroring `checkErrors`'
/// precedence. `None` when the page carries neither.
fn premium_only_or_wait_failure(html: &str, final_url: &str) -> Option<Failure> {
    if let Some(reason) = page::premium_only_reason(html, final_url) {
        return Some(
            Failure::coded(
                FailureKind::AuthRequired,
                messages::PREMIUM_ONLY,
                messages::premium_only(reason),
            )
            .with_param("reason", reason),
        );
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

/// The file name segment of a `/<code>/<name>` link.
fn second_path_segment(url: &Url) -> Option<String> {
    url.path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).nth(1))
        .map(str::to_owned)
}

/// Whether the account's premium period is still running.
///
/// `%Y-%m-%d %H:%M:%S`, as ddownload's API reports it. An unreadable value is not premium:
/// claiming premium on a date nobody can parse is the one answer that cannot be right.
fn premium_until(now_unix_seconds: u64, expiry: &str) -> bool {
    xfs_common::api::parse_expiry_unix(expiry)
        .is_some_and(|expiry| expiry > i64::try_from(now_unix_seconds).unwrap_or(i64::MAX))
}

/// The `Referer` a free transfer must carry, so the hoster sees the page that earned it.
pub(crate) fn referer_header() -> Header {
    Header::new("Referer", format!("https://{}/", api::PRIMARY_DOMAIN))
}

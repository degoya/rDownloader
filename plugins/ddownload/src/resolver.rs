//! DDownload's protocol logic, written once for both builds.
//!
//! Every function here takes the host as a parameter rather than reaching for one, so the same
//! code runs against the native `ResolverHost` and against the WIT imports. The two adapters
//! that supply it — `native.rs` and `guest.rs` — hold nothing but type conversions.
//!
//! Behaviour follows the native implementation, which was the richer of the two: it verifies a
//! cookie-only session instead of assuming it, computes premium status from the account's expiry
//! instead of asserting it, and tries the API's direct-link endpoint before falling back to the
//! HTML premium flow.
//!
//! An account is held in one of two ways, and which one the host admits is what
//! [`signs_in`] discovers. In `api_key` mode nothing changed: a stored key for the metadata API
//! plus, for downloads, a cookie session the user imported by hand. In `login` mode the account
//! stores a username and a password, and this plugin signs in for itself — the flow JDownloader
//! and pyLoad have always used, and the reason a user no longer has to copy a `Cookie:` header
//! out of browser devtools. The session that sign-in produces lives in the host's per-account
//! cookie jar, which this plugin can neither read nor write; it only has to notice when a
//! request came back unauthenticated and sign in once more.

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
    AccountResult, DirectLink, FileInfo, MATCH_HOSTS, api_request, api_request_with_key, coded,
    convert_envelope_error, ensure_http_status, file_code, file_name_from_disposition, invalid_url,
    is_html, parse_json, range_probe,
};
use crate::{messages, page};

/// The plugin's own display name, for the one log line a silent fallback leaves behind. A
/// constant, never a value off the wire: that is what keeps a key or an address out of a log.
const PROVIDER: &str = "DDownload";

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
    if signs_in(host, account_id).await {
        // A check must not undo the thing it is checking. Until RD-109-34 this signed in
        // unconditionally — "testing the account *is* signing in" — and [`sign_in`] begins by
        // demanding a sign-in form on the login page. Once the session exists, that fetch goes
        // out *as a signed-in user*, and the reported sequence is what follows: a sign-in that
        // worked, a download running on it, and seconds later a check reporting
        // `login_form_missing` about the very session that was delivering the file.
        //
        // So the session this plugin already holds is a result, not an obstacle. The account
        // page answers both questions in one request — whether there is a session, and the API
        // key that turns the check into measured figures — and it is the page a cold jar is
        // redirected away from (measured 2026-09-20: `GET /?op=my_account` without a session
        // answers 302 to `/login.html`, whose body reads as `Guest`). Signing in stays the
        // answer to "there is no session", which is JDownloader's order too:
        // `XFileSharingProBasic.loginWebsite` checks the site for a logged-in state before it
        // touches the login form.
        let mut page = signed_in_account_page(host).await?;
        let session = if page.signed_in() {
            Label::new().session_active()
        } else {
            sign_in(host).await?;
            page = signed_in_account_page(host).await?;
            Label::new().signed_in()
        };
        let Some(key) = page::api_key(&page.body) else {
            return Ok(Account {
                valid: true,
                // Nothing here measured the subscription. The sign-in proves the credentials
                // and the account page proves the session; neither says whether the account is
                // premium, and this used to answer `true` regardless — a free account was told
                // "Premium active" by the same code path. An unmeasured value is not a finding
                // (RD-109-34).
                premium: false,
                label: session.premium_unchecked().into(),
                traffic_left: None,
            });
        };
        let info = account_info_with_key(host, &key).await?;
        let premium = premium_until(host.now_unix_seconds().await, &info.premium_expire);
        return Ok(Account {
            valid: true,
            premium,
            label: Label::new().user(Some(&info.email)).into(),
            traffic_left: xfs_common::api::traffic_left_bytes(info.traffic_left),
        });
    }
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
        // A 200 says the site answered, not that it knows the session. Until RD-108-28 this
        // never looked at the body, so a lapsed cookie session stayed green until a download
        // had spent a captcha on it. Only a page offering the sign-out link is a session.
        //
        // It asks the account page, the one page with measured behaviour, through the same
        // request and verdict as the other two branches. Until RD-120-46 it asked the
        // homepage, which in DDownload's current design carries neither marker — the fault
        // RD-120-44 found in the `api_key` branch, so every cookie-only account lost its test.
        let page = signed_in_account_page(host).await?;
        match page.verdict {
            page::SessionVerdict::SignedIn => {}
            page::SessionVerdict::Guest => {
                let diagnosis = page::diagnose(&page.body);
                return Err(Failure::coded(
                    FailureKind::AccountInvalid,
                    messages::COOKIE_SESSION_INVALID,
                    messages::cookie_session_invalid(&diagnosis),
                )
                .with_param("diagnosis", diagnosis));
            }
            // The three answers stay three, and here the third is not the `api_key` branch's.
            // There a key has proven the account; here the session *is* the account and
            // nothing else proves it, so an unrecognized page cannot be a pass. Nor is it a
            // verdict against the account — the signed-in marker is still unmeasured, so a
            // signed-in page spelling its sign-out link differently would land here too. It is
            // reported as what it is: unconfirmed, retryable, and traced in the log.
            page::SessionVerdict::Unknown => {
                host.log(
                    "warn",
                    &crate::session_trace::unconfirmed_page_line(
                        PROVIDER,
                        "the account page",
                        &page.body,
                    ),
                );
                let diagnosis = page::diagnose(&page.body);
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
            // The sign-out link proves the session, and nothing here proves the subscription.
            // This answered `true` until RD-109-34, so an imported cookie session of a free
            // account was reported as "Premium active" exactly like a premium one.
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
    let label =
        verify_download_session(host, account_id, Label::new().user(Some(&info.email))).await?;
    Ok(Account {
        valid: true,
        premium,
        label: label.into(),
        traffic_left: xfs_common::api::traffic_left_bytes(info.traffic_left),
    })
}

/// Verifies the cookie session a download will run on, and adds what it found to `label`: the
/// number of cookies, and whether the site confirmed the session they carry.
///
/// This is the request the `api_key` branch of [`check_account`] used to leave out. The key
/// answers for the *account* — "Premium active, 195 GiB" — and says nothing whatever about the
/// browser session the premium flow needs, so the check counted the cookies instead: a session
/// the site had long forgotten read as "8 cookie(s) loaded for downloads", and the first
/// download met a captcha nobody could explain (RD-120-13).
///
/// It asks the account page, through [`signed_in_account_page`], and judges it by the verdict
/// the `login` branch acts on — one way to establish a session, not two. The first version
/// asked the homepage, which in DDownload's current design carries neither the sign-out link
/// nor the guest markup, so a working account failed its check with "neither signed in nor a
/// guest page" (RD-120-44). The account page is the one page with measured behaviour: a jar
/// without a session is redirected from it to `/login.html`, whose body reads as a guest.
///
/// The three verdicts weigh differently here than in the cookie-only branch, because here the
/// key has already proven the account:
///
/// - signed in: the session is confirmed, and the label says so;
/// - guest: the session has lapsed — a clear finding, reported as such, without condemning
///   the account;
/// - unknown: nothing was learned about the session, and not knowing is no reason to fail a
///   check the key has passed. The label says the session is unconfirmed.
///   What a signed-in account page looks like is still unmeasured, so this is also what a
///   signed-in page that spells its sign-out link differently would produce — which is why
///   the page is traced in the log, by title, length and markers, never by its body
///   (RD-120-46).
///
/// Zero cookies is deliberately not a refusal. Link checks run on the key alone, so an account
/// held for them is legitimate, and the label already states the absence in words rather than
/// leaving it to be inferred from a number.
async fn verify_download_session<H: PluginHost>(
    host: &H,
    account_id: &str,
    label: Label,
) -> Result<Label, Failure> {
    let cookies = cookie_count(host, account_id).await;
    let label = label.cookies(cookies);
    if cookies == 0 {
        return Ok(label);
    }
    let page = signed_in_account_page(host).await?;
    match page.verdict {
        // Only now, and only because a request came back saying so.
        page::SessionVerdict::SignedIn => Ok(label.session_active()),
        page::SessionVerdict::Guest => {
            let diagnosis = page::diagnose(&page.body);
            Err(Failure::coded(
                FailureKind::AuthRequired,
                messages::DOWNLOAD_SESSION_EXPIRED,
                messages::download_session_expired(&diagnosis),
            )
            .with_param("diagnosis", diagnosis))
        }
        page::SessionVerdict::Unknown => {
            host.log(
                "warn",
                &crate::session_trace::unconfirmed_page_line(
                    PROVIDER,
                    "the account page",
                    &page.body,
                ),
            );
            Ok(label.part(LabelPart::coded(
                messages::SESSION_UNCONFIRMED.0,
                messages::SESSION_UNCONFIRMED.1,
            )))
        }
    }
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
    let signs_in = signs_in(host, account_id).await;
    if !signs_in && cookie_count(host, account_id).await == 0 {
        // With a key in hand the account is an `api_key`-mode one missing its cookie session,
        // which is the older, narrower message. With neither slot answering, the account holds
        // no usable credential at all — in either mode — and saying "paste cookies" would send
        // the user back to the very thing the sign-in replaces.
        return Err(coded(
            FailureKind::AuthRequired,
            if has_api_key {
                messages::COOKIE_SESSION_REQUIRED_FOR_DOWNLOAD
            } else {
                messages::LOGIN_CREDENTIALS_REQUIRED
            },
        ));
    }
    let metadata = if has_api_key {
        file_info(host, &code).await?
    } else {
        None
    };
    let url_name = second_path_segment(&parsed);
    let mut transfer = premium_transfer(host, &request.url, &code, url_name.as_deref()).await?;
    // An HTML answer with no file attached means the premium flow was not served — most often
    // because the session lapsed or the process has not signed in yet. Sign in and try once
    // more, the way `XFileSharingProBasic.loginWebsite` re-authenticates on a failed cookie
    // check. Exactly once: a second failure is a real one, and this must not become a loop
    // that hammers the site's login form.
    if signs_in && no_file_delivered(&transfer) {
        sign_in(host).await?;
        transfer = premium_transfer(host, &request.url, &code, url_name.as_deref()).await?;
    }
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
    // The metadata API is the only thing that can answer a batched check, and it knows only
    // API keys. A `login`-mode account has none stored, so one is fetched off the signed-in
    // account page for the duration of this call.
    let scraped_key = if signs_in(host, account_id).await {
        // The same rule as in `check_account`: the session that is already there answers the
        // question, and signing in over it would fetch the login page as a signed-in user
        // (RD-109-34).
        let mut page = signed_in_account_page(host).await?;
        if !page.signed_in() {
            sign_in(host).await?;
            page = signed_in_account_page(host).await?;
        }
        Some(
            page::api_key(&page.body)
                .ok_or_else(|| coded(FailureKind::AuthRequired, messages::API_KEY_REQUIRED))?,
        )
    } else {
        if !host
            .secret_available(account_id, api::API_KEY_REFERENCE)
            .await
        {
            return Err(coded(FailureKind::AuthRequired, messages::API_KEY_REQUIRED));
        }
        None
    };
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
            let arguments = [("file_code", codes.join(","))];
            let request = match scraped_key.as_deref() {
                Some(key) => api_request_with_key("file/info", key, &arguments),
                None => api_request("file/info", &arguments),
            };
            let response = host.http(request).await?;
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
    account_info_request(host, api_request("account/info", &[])).await
}

/// The same call for a `login`-mode account, whose key was read off the account page.
async fn account_info_with_key<H: PluginHost>(
    host: &H,
    key: &str,
) -> Result<AccountResult, Failure> {
    account_info_request(host, api_request_with_key("account/info", key, &[])).await
}

async fn account_info_request<H: PluginHost>(
    host: &H,
    request: HttpRequest,
) -> Result<AccountResult, Failure> {
    let response = host.http(request).await?;
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

/// Some XFileSharing installations expose `file/direct_link` for premium API keys; DDownload
/// does not document it, so any failure falls back to the cookie flow.
///
/// The fallback stays quiet as far as the download is concerned, but it is no longer silent.
/// Four `.ok()?` in a row wrote nothing at all, which is why the running installation's error
/// log held not one line about ddownload while a user spent an evening on an expired session
/// (RD-120-13). The reason now goes out exactly once per attempt — one call, on the single
/// path that has an answer to report — and it is one of [`DirectLinkSkip`]'s fixed phrases, so
/// no file code, address or key can travel in it.
async fn direct_link<H: PluginHost>(host: &H, code: &str) -> Option<Resolved> {
    match direct_link_attempt(host, code).await {
        Ok(resolved) => Some(resolved),
        Err(skip) => {
            // `info`, not `warn`: for this provider the endpoint is expected to produce
            // nothing, so the fallback is normal and only its reason is diagnostic. `debug`
            // would have left it exactly as invisible as it was.
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
    if !url
        .host_str()
        .is_some_and(|host| host == api::PRIMARY_DOMAIN || host.ends_with(".ddownload.com"))
    {
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

/// Whether a premium attempt came back as a page rather than as the file itself.
///
/// The same condition the caller turns into `no_premium_file`; naming it separately is what
/// lets a sign-in be tried before that failure is reported.
fn no_file_delivered(response: &HttpResponse) -> bool {
    response.header("content-disposition").is_none() && is_html(response)
}

/// Whether this account signs in with stored credentials rather than carrying an API key.
///
/// The host answers `secret-available` per reference and admits only the slot the account's
/// credential mode selects, so this single probe is both "is there a password" and "which mode
/// is this account in" — the mode itself never crosses the sandbox boundary.
async fn signs_in<H: PluginHost>(host: &H, account_id: &str) -> bool {
    host.secret_available(account_id, api::PASSWORD_REFERENCE)
        .await
}

/// Signs in with the account's stored credentials.
///
/// Neither credential is visible here: the body carries the host's `{{username}}` and
/// `{{secret:…}}` markers and the host substitutes them, percent-encoded for the form body, on
/// the way out. The resulting `xfss` cookie is likewise invisible — the host's cookie jar keeps
/// it and replays it on this account's later requests.
async fn sign_in<H: PluginHost>(host: &H) -> Result<(), Failure> {
    let page = host.http(api::login_page_request()).await?;
    ensure_http_status(&page)?;
    let page_body = page.text().into_owned();
    // The login page fetched with a session in the jar is not a login page. `check_account` and
    // `check` ask the account page first and never get here with a session, but `resolve` signs
    // in when a premium attempt came back as a page, and that can happen while the session is
    // perfectly good. Reporting "no sign-in form" for a session that is already established is
    // the defect RD-109-34 was reported for; a site that answers this fetch with the customer
    // menu has answered the request, and the caller can carry on.
    if matches!(
        page::session_verdict(&page_body),
        page::SessionVerdict::SignedIn
    ) {
        return Ok(());
    }
    let form = page::login_form(&page_body).ok_or_else(|| {
        // Naming the page is the difference between "the site changed" and "this fetch was
        // answered with something else". RD-109-34 cost a measurement round trip because the
        // message said only that the form was absent.
        let diagnosis = page::diagnose(&page_body);
        Failure::coded(
            FailureKind::Permanent,
            messages::LOGIN_FORM_MISSING,
            messages::login_form_missing(&diagnosis),
        )
        .with_param("diagnosis", diagnosis)
    })?;
    // DDownload put Cloudflare Turnstile on this form after the sign-in shipped, and until it
    // was answered every attempt came back as "Wrong captcha" — the credentials were never even
    // read. The free flow has solved widget challenges through the host all along
    // (`resolver/free.rs`); the login path simply never had one to solve, so it never asked.
    let form = match page::login_challenge(&page_body) {
        Some(marker) => {
            let solution = host
                .solve_captcha(free::challenge_for(&marker, &page.final_url))
                .await?;
            page::with_challenge_token(&form, marker.kind, &solution.token)
        }
        None => form,
    };
    let action = form
        .action
        .clone()
        .unwrap_or_else(|| page.final_url.clone());
    let posted = host
        .http(
            HttpRequest::post(action, page::login_body(&form))
                .with_header("Content-Type", "application/x-www-form-urlencoded")
                .with_header("Referer", page.final_url.clone()),
        )
        .await?;
    ensure_http_status(&posted)?;
    match page::login_outcome(&api::set_cookies(&posted), &posted.text()) {
        xfs_common::login::LoginOutcome::Authenticated => Ok(()),
        // Wrong credentials will not become right by retrying; the account needs attention.
        xfs_common::login::LoginOutcome::BadCredentials => {
            Err(coded(FailureKind::AccountInvalid, messages::LOGIN_FAILED))
        }
        // The site refused this network, not this account, so the account stays valid.
        xfs_common::login::LoginOutcome::IpBlocked => {
            Err(coded(FailureKind::Transient(None), messages::LOGIN_BLOCKED))
        }
        // Nothing to retry and nothing the user can fix in the application: a browser challenge
        // needs a browser. Reported as `AccountInvalid` so the account is marked rather than
        // silently retried on every link.
        // The answer was produced and still refused. Nothing about the account is wrong, so
        // this is reported apart from bad credentials — sending someone to check a password
        // that was never read is the worst thing this path can do.
        xfs_common::login::LoginOutcome::CaptchaRejected(challenge) => Err(Failure::coded(
            FailureKind::Transient(None),
            messages::LOGIN_CAPTCHA,
            messages::login_captcha(challenge),
        )
        .with_param("challenge", challenge)),
        xfs_common::login::LoginOutcome::Unknown(reason) => Err(Failure::coded(
            FailureKind::AccountInvalid,
            messages::LOGIN_UNAVAILABLE,
            messages::login_unavailable(&reason),
        )
        .with_param("diagnosis", reason)),
    }
}

/// The account page and what it says about the session.
///
/// One request answers both questions a `login`-mode check has. Whether there is a session:
/// the page is what a signed-in visitor gets and what a visitor without a session is redirected
/// away from, so [`page::session_verdict`] reads it off the body. And the account's API key:
/// `check` and the traffic/expiry figures go through the metadata API, which knows only keys,
/// and a `login`-mode account has none stored — so it is read off this page the way
/// JDownloader's `DdownloadCom.findAPIKey` reads it. Nothing persists the key: a guest is
/// instantiated fresh for every call, so it lives exactly as long as the invocation.
///
/// The `api_key` branch asks the same page for the same reason — whether the imported cookie
/// session is alive — and reads the full verdict rather than [`AccountPage::signed_in`], because
/// there a guest page and an unrecognized one lead to different answers (RD-120-44).
struct AccountPage {
    body: String,
    /// What [`page::session_verdict`] read off the body.
    verdict: page::SessionVerdict,
}

impl AccountPage {
    /// The page carried the sign-out marker. Only [`page::SessionVerdict::SignedIn`] counts:
    /// a page that settles nothing is not a session, and signing in is what answers it.
    fn signed_in(&self) -> bool {
        matches!(self.verdict, page::SessionVerdict::SignedIn)
    }
}

async fn signed_in_account_page<H: PluginHost>(host: &H) -> Result<AccountPage, Failure> {
    let response = host.http(api::account_page_request()).await?;
    ensure_http_status(&response)?;
    let body = response.text().into_owned();
    let verdict = page::session_verdict(&body);
    Ok(AccountPage { body, verdict })
}

async fn cookie_count<H: PluginHost>(host: &H, account_id: &str) -> usize {
    host.cookies(account_id, &format!("https://{}/", api::PRIMARY_DOMAIN))
        .await
        .len()
}

/// Runs the XFileSharing premium flow: the file page carries a `download2` form that must be
/// posted with the logged-in cookie session; the answer is either the file (via redirect) or a
/// page carrying the direct link.
///
/// When the form is guarded by a captcha widget, the widget is answered through the host the
/// same way the free flow answers it, and the token goes into the submission. The file page
/// measured on 2026-09-17 (RD-108-28) carries a Cloudflare Turnstile inside the form; it was
/// fetched without a session, so whether a signed-in premium session is shown the widget too
/// is not known — hence a condition, not the rule: no widget, no token, the post as before.
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
    let Some(form) = page::download_form(&body) else {
        return Ok(page_response);
    };
    let mut submitted = page::premium_form(&form);
    if let Some(marker) = page::widget_marker(&body) {
        let solution = host
            .solve_captcha(free::challenge_for(&marker, &page_response.final_url))
            .await?;
        submitted = page::with_captcha_token(&submitted, marker.kind, &solution.token);
    }
    let posted = host
        .http(
            HttpRequest::post(
                page_response.final_url.clone(),
                page::encode_form(&submitted),
            )
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("Range", "bytes=0-0"),
        )
        .await?;
    ensure_http_status(&posted)?;
    if !is_html(&posted) {
        return Ok(posted);
    }
    let hints: Vec<&str> = url_name.into_iter().chain([code]).collect();
    let Some(link) = page::direct_link(&posted.text(), &hints) else {
        return Ok(posted);
    };
    Url::parse(&link).map_err(|error| invalid_url(&error))?;
    let transfer = host.http(range_probe(link)).await?;
    ensure_http_status(&transfer)?;
    Ok(transfer)
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

/// Re-exported for the adapters, which need the same header on a free transfer.
pub(crate) fn referer_header() -> Header {
    Header::new("Referer", format!("https://{}/", api::PRIMARY_DOMAIN))
}

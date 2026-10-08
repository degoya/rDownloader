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
mod login;

#[cfg(test)]
mod direct_link_tests;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LabelPart,
    LinkCheck, PluginHost, ResolveInput, Resolved, file_name_from_disposition,
};
use url::Url;
use xfs_common::site::{premium_until, second_path_segment};

use self::api::{
    MATCH_HOSTS, SITE, api_request, api_request_with_key, coded, ensure_http_status, file_code,
    invalid_url, is_html, range_probe,
};
use self::login::{no_file_delivered, sign_in, signed_in_account_page, signs_in};
use crate::{messages, page};

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    xfs_common::site::matches(url, MATCH_HOSTS)
}

/// Hoster domains this account can download from. A single hoster serves its own, so neither
/// the host nor the account changes the answer — the arguments are here because a multihoster's
/// catalogue does depend on both.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(plugin_common::own_hosters(crate::HOSTERS))
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
        let info = SITE
            .account_info(host, api_request_with_key("account/info", &key, &[]))
            .await?;
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
                    &xfs_common::session_trace::unconfirmed_page_line(
                        SITE.provider,
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
    let info = SITE
        .account_info(host, api_request("account/info", &[]))
        .await?;
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
    let cookies = SITE.cookie_count(host, account_id).await;
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
                &xfs_common::session_trace::unconfirmed_page_line(
                    SITE.provider,
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
    if has_api_key && let Some(resolved) = SITE.direct_link(host, &code).await {
        return Ok(resolved);
    }
    let signs_in = signs_in(host, account_id).await;
    if !signs_in && SITE.cookie_count(host, account_id).await == 0 {
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
        SITE.file_info(host, &code).await?
    } else {
        None
    };
    let url_name = second_path_segment(&parsed);
    let mut transfer = premium_transfer(host, &request.url, &code, url_name.as_deref()).await?;
    // An HTML answer with no file attached means the premium flow was not served — most often
    // because the session lapsed or the process has not signed in yet. Sign in and try once
    // more, the way `XFileSharingProBasic.loginWebsite` re-authenticates on a failed cookie
    // check. Exactly once: a second failure is a real one, and this must not become a loop
    // that hammers the site's login form. A used-up quota is no lapsed session, and signing in
    // again would only fetch the same answer (RD-1190-13).
    if signs_in && no_file_delivered(&transfer) && page::traffic_limit(&transfer.text()).is_none() {
        sign_in(host).await?;
        transfer = premium_transfer(host, &request.url, &code, url_name.as_deref()).await?;
    }
    let disposition = transfer.header("content-disposition").map(str::to_owned);
    if disposition.is_none() && is_html(&transfer) {
        if let Some(limit) = page::traffic_limit(&transfer.text()) {
            return Err(Failure::coded(
                FailureKind::RateLimited(Some(messages::TRAFFIC_EXHAUSTED_WAIT_SECONDS)),
                messages::TRAFFIC_EXHAUSTED,
                messages::traffic_exhausted(&limit),
            )
            .with_param("limit", limit));
        }
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
    SITE.link_checks(
        host,
        &request.urls,
        file_code,
        |path, arguments| match scraped_key.as_deref() {
            Some(key) => api_request_with_key(path, key, arguments),
            None => api_request(path, arguments),
        },
    )
    .await
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
            .solve_captcha(xfs_common::free::challenge_for(
                &marker,
                &page_response.final_url,
            ))
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

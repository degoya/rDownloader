//! The `login` credential mode: signing in with the account's stored credentials, and the
//! account page that says whether a session exists (split out of `resolver.rs`, RD-1120-10).

use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse, PluginHost};

use super::api::{self, coded, ensure_http_status, is_html};
use crate::{messages, page};

/// Whether a premium attempt came back as a page rather than as the file itself.
///
/// The same condition the caller turns into `no_premium_file`; naming it separately is what
/// lets a sign-in be tried before that failure is reported.
pub(super) fn no_file_delivered(response: &HttpResponse) -> bool {
    response.header("content-disposition").is_none() && is_html(response)
}

/// Whether this account signs in with stored credentials rather than carrying an API key.
///
/// The host answers `secret-available` per reference and admits only the slot the account's
/// credential mode selects, so this single probe is both "is there a password" and "which mode
/// is this account in" — the mode itself never crosses the sandbox boundary.
pub(super) async fn signs_in<H: PluginHost>(host: &H, account_id: &str) -> bool {
    host.secret_available(account_id, api::PASSWORD_REFERENCE)
        .await
}

/// Signs in with the account's stored credentials.
///
/// Neither credential is visible here: the body carries the host's `{{username}}` and
/// `{{secret:…}}` markers and the host substitutes them, percent-encoded for the form body, on
/// the way out. The resulting `xfss` cookie is likewise invisible — the host's cookie jar keeps
/// it and replays it on this account's later requests.
pub(super) async fn sign_in<H: PluginHost>(host: &H) -> Result<(), Failure> {
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
                .solve_captcha(xfs_common::free::challenge_for(&marker, &page.final_url))
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
pub(super) struct AccountPage {
    pub(super) body: String,
    /// What [`page::session_verdict`] read off the body.
    pub(super) verdict: page::SessionVerdict,
}

impl AccountPage {
    /// The page carried the sign-out marker. Only [`page::SessionVerdict::SignedIn`] counts:
    /// a page that settles nothing is not a session, and signing in is what answers it.
    pub(super) fn signed_in(&self) -> bool {
        matches!(self.verdict, page::SessionVerdict::SignedIn)
    }
}

pub(super) async fn signed_in_account_page<H: PluginHost>(
    host: &H,
) -> Result<AccountPage, Failure> {
    let response = host.http(api::account_page_request()).await?;
    ensure_http_status(&response)?;
    let body = response.text().into_owned();
    let verdict = page::session_verdict(&body);
    Ok(AccountPage { body, verdict })
}

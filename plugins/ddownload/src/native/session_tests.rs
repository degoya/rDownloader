//! What a green account test is worth, in `api_key` mode (RD-120-13, RD-120-44) and in the
//! cookie-only branch (RD-120-46).
//!
//! The reported sequence: a valid key, an expired cookie session, and a check that answered
//! "Premium active, 195 GiB, 8 cookie(s) loaded for downloads" — because it asked the API about
//! the account and then *counted* the jar. The download then met a captcha with nothing to
//! explain it. Every case here is answered from a prepared page, never from the network.
//!
//! And the correction that check needed (RD-120-44): it asked the homepage, which in
//! DDownload's current design carries neither marker, so a working account failed its test
//! with "neither signed in nor a guest page". It now asks the account page, judged the way
//! the `login` branch judges it, and an unrecognized page no longer fails a check the key
//! has passed.
//!
//! No signed-in account page has been recorded. [`SIGNED_IN_PAGE`] is not a capture: it is
//! the sign-out marker [`xfs_common::page::shows_signed_in`] believes, the same judgement the
//! `login` branch acts on, and nothing more.
//!
//! Split from `tests.rs` the way `premium_tests.rs` is, to keep that file under the line limit.

use std::sync::Arc;

use rd_core::AccountId;
use rd_plugin_api::{HostHttpResponse, ResolvedHeader, Resolver, ResolverHost};

use super::super::DdownloadResolver;
use super::{LOGIN_PAGE_2026_09_20, MockHost, html, json};

/// Where the session probe goes: the account page, as in the `login` branch.
const ACCOUNT_PAGE_URL: &str = "https://ddownload.com/?op=my_account";

/// The account endpoint's answer for the reported account: a key that works.
const ACCOUNT_INFO: &str = r#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2099-01-01 00:00:00","traffic_left":"204800"}}"#;

/// The three shapes the session probe can come back in. The sign-out link is the measured
/// positive marker; the header's `/login` link is what a lapsed session is served; an
/// interstitial carries neither and settles nothing.
const SIGNED_IN_PAGE: &str = r#"<title>DDownload</title><a href="/?op=logout">Logout</a>"#;
const EXPIRED_SESSION_PAGE: &str =
    r#"<title>DDownload</title><a class="nav-link" href="/login">Login</a>"#;
const UNREADABLE_PAGE: &str = r#"<title>Just a moment...</title><div id="cf-wrapper"></div>"#;

/// The page the owner's report names, reduced to what the report states about it: its title,
/// from the diagnosis `page "Ultimate Cloud Storage - DDownload" contains no premium link`,
/// and neither marker. Not a recording — the rest of that page was never captured.
const REPORTED_PAGE: &str =
    r#"<html><head><title>Ultimate Cloud Storage - DDownload</title></head><body></body></html>"#;

/// The account page answered for a jar without a session: the host follows the measured 302
/// to `/login.html`, and the body is the login page recorded on 2026-09-20.
fn redirected_to_login() -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: "https://ddownload.com/login.html".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: LOGIN_PAGE_2026_09_20.as_bytes().to_vec(),
    }
}

/// The reported case: the key answers, the session does not. It has its own code, apart from
/// `cookie_session_invalid` — the account is not what went wrong here — and the message names
/// the one thing that fixes it.
#[tokio::test]
async fn an_api_key_account_with_an_expired_cookie_session_reports_it() {
    let host = MockHost::with_responses(vec![json(ACCOUNT_INFO), html(EXPIRED_SESSION_PAGE)], true);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("an expired session must not read as a working account");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.download_session_expired")
    );
    assert!(
        failure.message.contains("cookie session"),
        "the message must say what to replace: {}",
        failure.message
    );
    assert!(
        failure.params.contains_key("diagnosis"),
        "the page that produced the verdict is named: {:?}",
        failure.params
    );
}

/// The other half: a session the site confirms stays green, and the label says the check
/// verified it rather than leaving a cookie count to imply it.
#[tokio::test]
async fn an_api_key_account_with_a_live_session_stays_green() {
    let host = MockHost::with_responses(vec![json(ACCOUNT_INFO), html(SIGNED_IN_PAGE)], true);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("a live session is a working account");
    assert!(status.valid);
    assert!(status.premium);
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.cookies(count=1)"), "{label}");
    assert!(label.contains("plugin.account.session_active()"), "{label}");

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2, "the session is verified with a request");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].url.as_str(),
        ACCOUNT_PAGE_URL,
        "the account page, not the homepage"
    );
}

/// The reported case (RD-120-44): the key answers, the account page is one this code does not
/// recognize. The key has proven the account, so the check passes with the API's figures, and
/// the label says the session was not confirmed rather than claiming or condemning it.
#[tokio::test]
async fn a_proven_key_passes_when_the_account_page_settles_nothing() {
    for unrecognized in [REPORTED_PAGE, UNREADABLE_PAGE] {
        let host = MockHost::with_responses(vec![json(ACCOUNT_INFO), html(unrecognized)], true);
        let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
        let status = resolver
            .check_account(AccountId::new())
            .await
            .expect("an unconfirmed session does not fail an account the key has proven");
        assert!(status.valid);
        assert!(status.premium, "premium comes from the API");
        assert_eq!(
            status.traffic_left.map(rd_core::ByteCount::get),
            Some(204_800 * 1024 * 1024),
            "the volume comes from the API"
        );
        let label = plugin_common::native::label_summary(&status.label);
        assert!(label.contains("plugin.account.cookies(count=1)"), "{label}");
        assert!(label.contains("ddownload.session_unconfirmed"), "{label}");
        assert!(
            !label.contains("plugin.account.session_active()"),
            "nothing confirmed the session, so nothing claims it: {label}"
        );
        let requests = host.requests.lock().expect("mock lock");
        assert_eq!(requests[1].url.as_str(), ACCOUNT_PAGE_URL);
    }
}

/// The one account-page behaviour that was measured (2026-09-20): without a session the site
/// redirects `/?op=my_account` to `/login.html`. That is a lapsed session, and it stays a clear
/// finding — read off the recorded login page, not a page made up for the test.
#[tokio::test]
async fn the_redirect_to_the_login_page_is_an_expired_session() {
    let host = MockHost::with_responses(vec![json(ACCOUNT_INFO), redirected_to_login()], true);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("the login page is not a session");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.download_session_expired")
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[1].url.as_str(), ACCOUNT_PAGE_URL);
}

/// An account held for link checks alone has no session to verify, and that is not a failure:
/// `check` runs on the key. The label states the absence in words, and no probe goes out.
#[tokio::test]
async fn an_api_key_account_without_cookies_is_reported_without_a_probe() {
    let host = MockHost::full(vec![json(ACCOUNT_INFO)], true, false);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("a key-only account is still an account");
    assert!(status.valid);
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.cookies(count=0)"), "{label}");
    assert!(
        !label.contains("plugin.account.session_active()"),
        "nothing was verified, so nothing claims it: {label}"
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

// --- the cookie-only branch (RD-120-46) -----------------------------------------------------

/// Nothing but the session proves a cookie-only account, and it is now asked on the account
/// page as well. It used to ask the homepage, which in DDownload's current design carries
/// neither marker, so every such account lost its test.
#[tokio::test]
async fn a_cookie_only_account_is_verified_on_the_account_page() {
    let host = MockHost::with_responses(vec![html(SIGNED_IN_PAGE)], false);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("a signed-in account page is a session");
    assert!(status.valid);
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.session_active()"), "{label}");
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.as_str(),
        ACCOUNT_PAGE_URL,
        "the account page, not the homepage"
    );
}

/// The measured redirect to `/login.html` is a lapsed session, and here the session is the
/// account: the same clear finding the branch has always reported for a guest page.
#[tokio::test]
async fn a_cookie_only_account_redirected_to_the_login_page_is_refused() {
    let host = MockHost::with_responses(vec![redirected_to_login()], false);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("the login page is not a session");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.cookie_session_invalid")
    );
}

/// The reported page in the cookie-only branch: nothing else proves the account, so it is
/// neither a pass nor a verdict against it — reported as unconfirmed and retryable.
#[tokio::test]
async fn a_cookie_only_account_on_an_unrecognized_page_is_reported_unconfirmed() {
    let host = MockHost::with_responses(vec![html(REPORTED_PAGE)], false);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("an unrecognized page proves no cookie-only account");
    assert_eq!(
        failure.category,
        rd_core::FailureKind::Transient {
            retry_after_seconds: None
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.cookie_session_unconfirmed")
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[0].url.as_str(), ACCOUNT_PAGE_URL);
}

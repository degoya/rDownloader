//! Premium resolve coverage for the file page DDownload serves since 2026-09-17: the
//! `download2` form directly on the page with a Turnstile widget inside it (RD-108-28). Split
//! from `tests.rs` the way `free_tests.rs` is, to keep that file under the line limit.

use std::sync::Arc;

use rd_core::AccountId;
use rd_plugin_api::{CaptchaChallenge, Resolver, ResolverHost};

use super::super::DdownloadResolver;
use super::{
    FILE_PAGE_2026_09_17, FORM_PAGE, MockHost, TURNSTILE_SITE_KEY, file,
    file_page_without_the_form, html, resolve_request,
};

/// The file page of 2026-09-17 carries a Turnstile widget inside the `download2` form. The
/// premium flow found the form and posted it without a token, got a page back, and reported a
/// login problem the account did not have. The widget is answered the way the free flow answers
/// it, and the token travels in the same submission as `method_premium`.
#[tokio::test]
async fn the_premium_flow_answers_the_turnstile_on_the_file_page() {
    let host = MockHost::cookie_session_with_solver(
        vec![
            html(FILE_PAGE_2026_09_17),
            file("https://eu-hydra5.zeuscdn.org:183/d/tok3n/release.rar"),
        ],
        "turnstile-token",
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);

    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://eu-hydra5.zeuscdn.org:183/d/tok3n/release.rar"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2, "page, download2");
    assert_eq!(requests[1].method, "POST");
    let body = String::from_utf8_lossy(&requests[1].body);
    assert!(body.contains("op=download2"), "{body}");
    assert!(body.contains("id=og21jsivxm1m"), "{body}");
    assert!(
        body.contains("rand=fq5eytcd4ppxoxbmje4ld2eaf6dwnypjnrsev44mza"),
        "{body}"
    );
    assert!(body.contains("method_premium=Premium+Download"), "{body}");
    assert!(!body.contains("method_free"), "{body}");
    assert!(
        body.contains("cf-turnstile-response=turnstile-token"),
        "{body}"
    );
    let captchas = host.captchas.lock().expect("mock lock");
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, TURNSTILE_SITE_KEY);
            assert_eq!(
                widget.page_url,
                "https://ddownload.com/abc123xyz/release.rar"
            );
        }
        other => panic!("expected a Turnstile challenge, got {other:?}"),
    }
    assert!(
        host.waits.lock().expect("mock lock").is_empty(),
        "a premium session does not wait out the free countdown"
    );
}

/// A form without a widget is posted exactly as before: no solver is asked, no token is sent.
#[tokio::test]
async fn a_form_without_a_widget_is_posted_without_a_token() {
    let host = MockHost::cookie_session_with_solver(
        vec![
            html(FORM_PAGE),
            file("https://fs7.ddownload.com/d/r4nd/release.rar"),
        ],
        "unused",
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);

    resolver.resolve(resolve_request()).await.expect("resolved");

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[1].body,
        b"op=download2&id=abc123xyz&rand=r4nd&method_premium=Premium+Download"
    );
    assert!(host.captchas.lock().expect("mock lock").is_empty());
}

/// The answer page carries the header's login link, as every page a guest sees does. That is
/// not a login wall, and the message must not send the user after their cookies.
#[tokio::test]
async fn a_premium_answer_with_only_the_navigation_link_is_not_called_a_login_wall() {
    let host = MockHost::with_responses(
        vec![html(FORM_PAGE), html(&file_page_without_the_form())],
        false,
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);

    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("no file came back");

    assert_eq!(failure.code.as_deref(), Some("ddownload.no_premium_file"));
    let diagnosis = failure
        .params
        .get("diagnosis")
        .expect("the diagnosis travels with the failure");
    assert!(
        !diagnosis.contains("requires a login"),
        "a header link is not a login wall: {diagnosis}"
    );
    assert_eq!(
        diagnosis,
        "page \"Download Adults 2025 S02E01 GERMAN WEBRiP x264 4SJ rar\" contains no premium link"
    );
}

// --- the cookie-only account check ---------------------------------------------------------

/// A cookie-only account is verified against the site: only a page offering the sign-out link
/// is a session. Until RD-108-28 the body was never read, so a lapsed session stayed green
/// until a download had spent a captcha on it.
#[tokio::test]
async fn a_cookie_session_is_believed_only_when_the_site_shows_it_signed_in() {
    let signed_in = MockHost::with_responses(
        vec![html(
            r#"<a href="/?op=logout">Logout</a><div class="rm-login-link"><a href="/login">Login</a></div>"#,
        )],
        false,
    );
    let resolver = DdownloadResolver::new(Arc::clone(&signed_in) as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("a signed-in page is a session");
    assert!(account.valid);
    assert!(
        plugin_common::native::label_summary(&account.label)
            .contains("plugin.account.cookies(count=1)")
    );
}

/// The guest page - the file page of 2026-09-17 without its form, i.e. the header with its
/// login links and no sign-out link - is a lapsed session, and is reported as one.
#[tokio::test]
async fn a_lapsed_cookie_session_is_reported_before_any_download_spends_a_captcha() {
    let host = MockHost::with_responses(vec![html(&file_page_without_the_form())], false);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("a guest page is not a session");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.cookie_session_invalid")
    );
    assert!(
        failure.message.contains("not signed in"),
        "{}",
        failure.message
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

/// A page that is neither signed in nor a guest page says nothing about the account, and must
/// not be read as if it did. This is the review finding of RD-108-28: the first fix turned
/// every unrecognized page into `AccountInvalid` with "paste a fresh cookie session", so a
/// Cloudflare interstitial or a maintenance notice would have condemned the premium account
/// the bug report says works — and whether the signed-in page even carries the English
/// sign-out marker is still unmeasured (checklist D).
#[tokio::test]
async fn an_unreadable_page_is_retried_and_does_not_condemn_the_account() {
    for unreadable in [
        "<title>Just a moment...</title><div id=\"cf-wrapper\"></div>",
        "<title>Maintenance</title><p>We are back shortly.</p>",
    ] {
        let host = MockHost::with_responses(vec![html(unreadable)], false);
        let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
        let failure = resolver
            .check_account(AccountId::new())
            .await
            .expect_err("an unreadable page confirms nothing either way");
        assert_eq!(
            failure.category,
            rd_core::FailureKind::Transient {
                retry_after_seconds: None
            },
            "{unreadable} must be retryable, not an account fault"
        );
        assert_eq!(
            failure.code.as_deref(),
            Some("ddownload.cookie_session_unconfirmed"),
            "{unreadable}"
        );
        assert!(
            !failure.message.contains("paste a fresh cookie session"),
            "the user must not be sent after cookies that are not the problem: {}",
            failure.message
        );
    }
}

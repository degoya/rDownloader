//! Signing in with stored credentials (credential mode `login`), the flow that replaces
//! copying a `Cookie:` header out of browser devtools.

use std::sync::Arc;

use rd_core::AccountId;
use rd_plugin_api::{Resolver, ResolverHost};

use super::super::DdownloadResolver;
use super::{FORM_PAGE, LOGIN_PAGE_2026_09_20, MockHost, file, html, json, resolve_request};

/// The login page as ddownload serves it, trimmed to the form the sign-in submits.
const LOGIN_PAGE: &str = r#"<form method="POST" action="https://ddownload.com/" name="FL">
<input type="hidden" name="op" value="login">
<input type="hidden" name="token" value="09509f485f267c36c2b54fa96330c2b3">
<input type="hidden" name="redirect" value="">
<input type="text" name="login" value="" required>
<input type="password" name="password" required>
</form>"#;

/// The page a signed-in visitor gets: no login wall, so [`login_outcome`] confirms the session
/// even though the mock sets no cookie.
const SIGNED_IN_PAGE: &str = r#"<html><body><a href="/?op=logout">Log out</a></body></html>"#;

/// The account page of a signed-in visitor, with the API key in the read-only field the XFS
/// template renders it in.
const ACCOUNT_PAGE_WITH_KEY: &str = r#"<html><body><a href="/?op=logout">Log out</a>
<input type="text" value="0123456789abcdef01" readonly></body></html>"#;

/// The metadata API's answer for a premium account.
const PREMIUM_ACCOUNT_JSON: &str = r#"{"status":200,"result":{"email":"user@example.test","premium_expire":"2099-01-01 00:00:00","balance":"0","storage_used":"0","traffic_left":"12345"}}"#;

#[tokio::test]
async fn a_login_account_signs_in_and_retries_the_premium_flow() {
    // First attempt yields a page rather than a file, which is what an unauthenticated premium
    // attempt looks like; then the login page, the sign-in answer, and the retry.
    let host = MockHost::signing_in(vec![
        html("<html><body>Please log in</body></html>"),
        html(LOGIN_PAGE),
        html(SIGNED_IN_PAGE),
        html(FORM_PAGE),
        file("https://fs7.ddownload.com/d/r4nd/release.rar"),
    ]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.ddownload.com/d/r4nd/release.rar"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[1].url.as_str(),
        "https://ddownload.com/login.html",
        "the sign-in starts at the login page"
    );
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].url.as_str(), "https://ddownload.com/");
    // The credentials travel as markers; this plugin never holds either value.
    assert_eq!(
        requests[2].body,
        b"op=login&token=09509f485f267c36c2b54fa96330c2b3&redirect=\
          &login={{username}}&password={{secret:ddownload_password}}"
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<u8>>()
    );
}

/// A sign-in the site did not accept redirects to the homepage: no login form, no cookie, but
/// the guest header with its login link. That must not be read as a session — it would show a
/// green account and then fail every download behind it.
#[tokio::test]
async fn a_sign_in_answered_with_a_guest_page_is_not_believed() {
    let guest_home = r#"<html><head><title>DDownload</title></head><body>
<a class="nav-link outlined" href="/login">Login</a></body></html>"#;
    // The account page first (a cold jar is redirected to the login page), then the sign-in.
    let host = MockHost::signing_in(vec![html(LOGIN_PAGE), html(LOGIN_PAGE), html(guest_home)]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("a guest page is not a session");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("ddownload.login_unavailable"));
    assert!(
        failure.message.contains("guest header"),
        "{}",
        failure.message
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 3);
}

/// A cold cookie jar is the normal state after a restart, and it must not cost the user an
/// error — the plugin signs in instead. This is the whole point of the feature.
#[tokio::test]
async fn a_login_account_needs_no_imported_cookies() {
    let host = MockHost::signing_in(vec![
        html("<html><body>Please log in</body></html>"),
        html(LOGIN_PAGE),
        html(SIGNED_IN_PAGE),
        html(FORM_PAGE),
        file("https://fs7.ddownload.com/d/r4nd/release.rar"),
    ]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    resolver
        .resolve(resolve_request())
        .await
        .expect("no cookie session is required in login mode");
}

#[tokio::test]
async fn wrong_credentials_invalidate_the_account_rather_than_being_retried() {
    let host = MockHost::signing_in(vec![
        html("<html><body>Please log in</body></html>"),
        html(LOGIN_PAGE),
        html(&format!(
            "<div class=\"err\">Incorrect Login or Password</div>{LOGIN_PAGE}"
        )),
    ]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("wrong credentials");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("ddownload.login_failed"));
}

/// A blocked IP is the site refusing this network, not these credentials; reporting it as an
/// invalid account would disable an account that is perfectly good.
#[tokio::test]
async fn a_blocked_ip_is_transient_and_leaves_the_account_valid() {
    let host = MockHost::signing_in(vec![
        html("<html><body>Please log in</body></html>"),
        html(LOGIN_PAGE),
        html(&format!(
            "<div class=\"err\">Your IP is banned</div>{LOGIN_PAGE}"
        )),
    ]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("blocked IP");
    assert!(matches!(
        failure.category,
        rd_core::FailureKind::Transient { .. }
    ));
    assert_eq!(failure.code.as_deref(), Some("ddownload.login_blocked"));
}

/// Only once. A second failure is a real one, and retrying in a loop would hammer the site's
/// login form.
#[tokio::test]
async fn the_sign_in_retry_happens_at_most_once() {
    let host = MockHost::signing_in(vec![
        html("<html><body>Please log in</body></html>"),
        html(LOGIN_PAGE),
        html(SIGNED_IN_PAGE),
        html("<html><title>Still not a file</title></html>"),
    ]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("still no file");
    assert_eq!(failure.code.as_deref(), Some("ddownload.no_premium_file"));
    assert_eq!(
        host.requests.lock().expect("mock lock").len(),
        4,
        "one attempt, one sign-in, one retry -- and then it stops"
    );
}

#[tokio::test]
async fn testing_a_login_account_signs_in_and_reads_the_api_key_off_the_account_page() {
    let host = MockHost::signing_in(vec![
        html(LOGIN_PAGE),
        html(LOGIN_PAGE),
        html(SIGNED_IN_PAGE),
        html(
            r#"<a href="/?op=logout">Log out</a><input type="text" value="0123456789abcdef01" readonly>"#,
        ),
        json(PREMIUM_ACCOUNT_JSON),
    ]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("account");
    assert!(account.valid);
    assert!(account.premium);
    assert_eq!(
        plugin_common::native::label_summary(&account.label),
        "plugin.account.user(user=user@example.test)"
    );
    let requests = host.requests.lock().expect("mock lock");
    // The scraped key travels as a value, not as a marker: there is no stored slot to expand.
    assert!(
        requests[4]
            .query
            .iter()
            .any(|value| value.name == "key" && value.value_template == "0123456789abcdef01"),
        "the account info call carries the scraped key"
    );
}

/// Without a key there is nothing to ask the metadata API, but the sign-in itself already
/// proves the credentials, so the account is reported as usable rather than broken.
#[tokio::test]
async fn testing_a_login_account_without_a_reachable_api_key_still_reports_the_account() {
    let host = MockHost::signing_in(vec![
        html(LOGIN_PAGE),
        html(LOGIN_PAGE),
        html(SIGNED_IN_PAGE),
        html(r#"<a href="/?op=logout">Log out</a>no key on this page"#),
    ]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("account");
    assert!(account.valid);
    assert!(account.traffic_left.is_none());
}

/// An account whose secret slot is empty holds no usable credential in either mode; telling
/// such a user to paste cookies would point at exactly what the sign-in replaces.
#[tokio::test]
async fn an_account_with_no_credential_at_all_is_told_what_it_needs() {
    let host = MockHost::full(Vec::new(), false, false);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("no credential");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.login_credentials_required")
    );
}

// --- the check and the session it already holds (RD-109-34) ---------------------------------

/// The reported defect, in the state the host is really in after a sign-in: the session lives
/// in the host's cookie jar, downloads run through it — and the check used to fetch the login
/// page anyway, as a signed-in user, and report `login_form_missing` about a session that was
/// delivering a file at that moment.
///
/// The session is the result the check is after. It asks the account page for it, and only a
/// page that says there is no session sends it to the login form.
#[tokio::test]
async fn a_check_on_an_established_session_reports_it_instead_of_signing_in_again() {
    let host = MockHost::signed_in_session(vec![
        html(ACCOUNT_PAGE_WITH_KEY),
        json(PREMIUM_ACCOUNT_JSON),
    ]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("an established session is a result, not a reason to fail the check");
    assert!(account.valid);
    assert!(account.premium, "the metadata API reported the expiry");
    assert_eq!(
        plugin_common::native::label_summary(&account.label),
        "plugin.account.user(user=user@example.test)"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[0].url.as_str(),
        "https://ddownload.com/?op=my_account",
        "the check asks the session it has before making a new one"
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.url.as_str().contains("login.html")),
        "no request may ask for the login form while the session stands"
    );
    assert_eq!(requests.len(), 2, "one page, one metadata call");
}

/// The same for the batched link check, which scrapes the key off the same page and used to
/// sign in for it unconditionally.
#[tokio::test]
async fn a_link_check_on_an_established_session_does_not_sign_in_again() {
    let host = MockHost::signed_in_session(vec![
        html(ACCOUNT_PAGE_WITH_KEY),
        json(
            r#"{"status":200,"result":[{"status":200,"filecode":"abc123xyz","name":"release.rar","size":"12"}]}"#,
        ),
    ]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let checked = resolver
        .check(rd_plugin_api::CheckRequest {
            urls: vec![
                "https://ddownload.com/abc123xyz/release.rar"
                    .parse()
                    .expect("URL"),
            ],
            client: rd_plugin_api::ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("the session answers the check");
    assert_eq!(checked.len(), 1);
    let requests = host.requests.lock().expect("mock lock");
    assert!(
        requests
            .iter()
            .all(|request| !request.url.as_str().contains("login.html")),
        "no request may ask for the login form while the session stands"
    );
}

/// The cold jar, against the login page as the site really served it on 2026-09-20: the account
/// page redirects a visitor without a session to it, it reads as a guest page, and the check
/// signs in — through the Turnstile widget the form has carried since 2026-09-17.
#[tokio::test]
async fn a_cold_jar_still_signs_in_through_the_page_the_site_really_serves() {
    let host = MockHost::signing_in_with_solver(
        vec![
            html(LOGIN_PAGE_2026_09_20),
            html(LOGIN_PAGE_2026_09_20),
            html(SIGNED_IN_PAGE),
            html(ACCOUNT_PAGE_WITH_KEY),
            json(PREMIUM_ACCOUNT_JSON),
        ],
        "turnstile-token",
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("a cold jar signs in");
    assert_eq!(
        plugin_common::native::label_summary(&account.label),
        "plugin.account.user(user=user@example.test)"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[0].url.as_str(),
        "https://ddownload.com/?op=my_account"
    );
    assert_eq!(requests[1].url.as_str(), "https://ddownload.com/login.html");
    assert_eq!(requests[2].method, "POST");
    let captchas = host.captchas.lock().expect("mock lock");
    assert_eq!(
        captchas.len(),
        1,
        "the measured login form carries a widget"
    );
}

/// A premium attempt that came back as a page is a reason to sign in, and the session may be
/// good all the same. The login page fetched with that session is not a login page, and saying
/// "no sign-in form" about it is the message this job was reported for — the caller carries on
/// and reports what the premium flow really answered.
#[tokio::test]
async fn a_sign_in_over_a_standing_session_is_not_a_missing_form() {
    let host = MockHost::signed_in_session(vec![
        html("<html><body>Please log in</body></html>"),
        html(SIGNED_IN_PAGE),
        html("<html><title>Still not a file</title></html>"),
    ]);
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("the premium flow still delivered no file");
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.no_premium_file"),
        "the premium answer is the finding, not an invented login problem"
    );
}

/// A login page that really is not one now names what arrived instead. RD-109-34 cost a
/// measurement round trip because the message said only that the form was absent.
#[tokio::test]
async fn a_login_page_without_a_form_says_what_the_page_was() {
    // The account page answers with the interstitial too, so the check does go on to sign in:
    // a page that settles nothing is not a session.
    let interstitial = "<title>Just a moment...</title><div id=\"cf-wrapper\"></div>";
    let host = MockHost::signing_in(vec![html(interstitial), html(interstitial)]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("an interstitial carries no sign-in form");
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.login_form_missing")
    );
    assert!(
        failure.message.contains("Just a moment"),
        "the page names itself; the old message said only that the form was absent: {}",
        failure.message
    );
}

// --- premium is reported only where it was measured (RD-109-34) ------------------------------

/// The hard-coded finding. A sign-in proves the credentials and the account page proves the
/// session; neither says anything about the subscription, and this branch answered `premium:
/// true` regardless — a free account was shown "Premium active" by the identical path.
#[tokio::test]
async fn a_session_without_a_readable_api_key_does_not_claim_premium() {
    let host = MockHost::signed_in_session(vec![html(SIGNED_IN_PAGE)]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("the session is established");
    assert!(account.valid, "the session was proven");
    assert!(
        !account.premium,
        "nothing in this path measured the subscription"
    );
    assert!(
        plugin_common::native::label_summary(&account.label)
            .contains("plugin.account.premium_unchecked()"),
        "{}",
        plugin_common::native::label_summary(&account.label)
    );
}

/// The same rule for an imported cookie session: the sign-out link proves the session, and the
/// subscription stays unmeasured.
#[tokio::test]
async fn a_cookie_only_session_does_not_claim_premium_either() {
    let host = MockHost::with_responses(vec![html(SIGNED_IN_PAGE)], false);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("the cookie session is established");
    assert!(account.valid);
    assert!(!account.premium, "nobody read a subscription off that page");
    assert!(
        plugin_common::native::label_summary(&account.label)
            .contains("plugin.account.premium_unchecked()"),
        "{}",
        plugin_common::native::label_summary(&account.label)
    );
}

/// Premium where it *was* measured stays premium: the metadata API reports the expiry, and an
/// expiry in the future is a finding.
#[tokio::test]
async fn a_measured_expiry_still_reports_premium() {
    let host = MockHost::signed_in_session(vec![
        html(ACCOUNT_PAGE_WITH_KEY),
        json(PREMIUM_ACCOUNT_JSON),
    ]);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let account = resolver
        .check_account(AccountId::new())
        .await
        .expect("account");
    assert!(account.premium);
}

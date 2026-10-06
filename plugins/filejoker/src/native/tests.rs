//! MockHost suite cloned from `plugins/ddownload/src/native/tests.rs`'s harness pattern, covering
//! the flows this task brief calls out explicitly: the cookie-missing gate on both `resolve` and
//! `check_account` (asserting zero requests), the happy premium-form flow, a direct-file response,
//! a login-wall page (`filejoker.session_invalid`), an offline page (`Offline`), the captcha
//! short-circuit (scoped to the form, per Task 11's finding), the premium-only and wait markers,
//! an unrecognized-page fallback (`filejoker.page_error`), HTTP-status mapping (401 and 429), and
//! the `check()`/`hosters()` trait defaults (`plugins/debridlink/src/native/tests.rs`'s
//! `check_defaults_to_unsupported` pattern: asserting the trait default's actual output against
//! the same constant `guest.rs` reproduces by hand keeps native and guest from silently
//! desyncing).

use std::sync::Arc;

use rd_core::{AccountId, FailureKind};
use rd_plugin_api::test_support::ScriptedHost as MockHost;
use rd_plugin_api::{
    ClientIdentity, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver, ResolverHost,
};

use super::FilejokerResolver;

/// FileJoker's constructors on the shared scripted host: a premium account's cookie session in
/// the jar, or no jar at all. The free flow is `MockHost::free`, which never answers a cookie
/// lookup — `cookies_asked` records whether one was made.
trait SessionHost {
    fn one_in_session(response: HostHttpResponse) -> Arc<MockHost>;
    fn in_session(responses: Vec<HostHttpResponse>) -> Arc<MockHost>;
    fn without_cookies() -> Arc<MockHost>;
}

impl SessionHost for MockHost {
    fn one_in_session(response: HostHttpResponse) -> Arc<MockHost> {
        Self::in_session(vec![response])
    }

    fn in_session(responses: Vec<HostHttpResponse>) -> Arc<MockHost> {
        MockHost::scripted(responses)
            .cookies(&[("xfss", "session")])
            .shared()
    }

    fn without_cookies() -> Arc<MockHost> {
        MockHost::answering(Vec::new())
    }
}

fn html(body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: "https://filejoker.net/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn file(final_url: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 206,
        final_url: final_url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Disposition".to_owned(),
            value: "attachment; filename=release.rar".to_owned(),
        }],
        body: vec![0],
    }
}

fn status_only(url: &str, status: u16) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: url.parse().expect("URL"),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

const FORM_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_premium" value="">
</form>"#;

const CAPTCHA_FORM_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_premium" value="">
<div class="g-recaptcha" data-sitekey="6Lc-site-key"></div>
</form>"#;

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://filejoker.net/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

// --- domains ---------------------------------------------------------------------------------

#[test]
fn matches_the_only_domain_and_rejects_others() {
    let resolver = FilejokerResolver::new(MockHost::without_cookies());
    assert!(resolver.matches(&"https://filejoker.net/abc123xyz".parse().expect("URL")));
    assert!(resolver.matches(&"https://www.filejoker.net/abc123xyz".parse().expect("URL")));
    assert!(!resolver.matches(&"https://example.com/abc123xyz".parse().expect("URL")));
    assert!(!resolver.matches(&"https://filejoker.net/short".parse().expect("URL")));
}

// --- cookie gate -------------------------------------------------------------------------------

#[tokio::test]
async fn resolve_without_cookies_makes_no_requests() {
    let host = MockHost::without_cookies();
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("missing cookies must fail");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("filejoker.cookies_missing"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 0);
}

#[tokio::test]
async fn check_account_without_cookies_makes_no_requests() {
    let host = MockHost::without_cookies();
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing cookies must fail");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("filejoker.cookies_missing"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 0);
}

// --- check_account -------------------------------------------------------------------------

#[tokio::test]
async fn check_account_reports_reachable_cookie_session() {
    // The sign-out link is what proves the session; a bare title would not.
    let response = html("<title>FileJoker</title><a href=\"/?op=logout\">Logout</a>");
    let host = MockHost::one_in_session(response);
    let resolver = FilejokerResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("cookie session reachable");
    assert!(status.valid);
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.cookies(count=1)"), "{label}");
    // The count alone is what made a green check meaningless in the plugins that had an API
    // key to hide behind (RD-120-13). This branch has verified the session all along; since
    // that job it also says so, so the count never stands as the only thing a green test shows.
    assert!(label.contains("plugin.account.session_active()"), "{label}");

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.as_str(), "https://filejoker.net/");
}

/// The sign-out link proves the session, not the subscription. FileJoker has no metadata API —
/// no expiry, no plan, no traffic figure — so no branch of this plugin reads one, and this is
/// the only branch that reports an account at all. Until RD-109-38 it answered `premium: true`
/// and the interface printed "Premium active" for a free account with working cookies.
#[tokio::test]
async fn a_cookie_only_session_does_not_claim_premium() {
    let response = html("<title>FileJoker</title><a href=\"/?op=logout\">Logout</a>");
    let resolver = FilejokerResolver::new(MockHost::one_in_session(response));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("cookie session reachable");
    assert!(status.valid);
    assert!(!status.premium, "nothing here read the subscription");
    assert!(
        plugin_common::native::label_summary(&status.label)
            .contains("plugin.account.premium_unchecked()"),
        "the label must say what was not read: {}",
        plugin_common::native::label_summary(&status.label)
    );
}

#[tokio::test]
async fn check_account_reports_session_invalid_on_login_wall() {
    // The sign-in form, not the header's `/login` link: the link is on every guest page.
    let response = html(
        r#"<form method="POST" name="FL"><input type="hidden" name="op" value="login"></form>"#,
    );
    let resolver = FilejokerResolver::new(MockHost::one_in_session(response));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("login wall must fail");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("filejoker.session_invalid"));
    assert!(failure.message.contains("logging in again"));
}

/// A lapsed session lands on a guest page: no login form, no sign-out link, but the header
/// still offers the login link. That is not a healthy session, and it must not be reported
/// as one.
#[tokio::test]
async fn check_account_does_not_believe_a_guest_page_without_a_login_form() {
    let response = html("<title>FileJoker</title><a class=\"nav-link\" href=\"/login\">Login</a>");
    let resolver = FilejokerResolver::new(MockHost::one_in_session(response));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("a guest page is not a session");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("filejoker.session_invalid"));
}

/// A page carrying neither marker is not an account problem, and this is the assertion that
/// changed: the first fix of RD-108-28 reported `session_invalid` here too, so a maintenance
/// page or a Cloudflare interstitial told the user their working premium account was invalid
/// and sent them after their cookies. FileJoker's own pages are unmeasured, which makes the
/// unrecognized page the likely one, not the exotic one.
#[tokio::test]
async fn check_account_does_not_condemn_an_account_over_a_page_it_cannot_read() {
    for unreadable in [
        "<title>Maintenance</title>",
        "<title>Just a moment...</title><div id=\"cf-wrapper\"></div>",
    ] {
        let resolver = FilejokerResolver::new(MockHost::one_in_session(html(unreadable)));
        let failure = resolver
            .check_account(AccountId::new())
            .await
            .expect_err("an unreadable page confirms nothing");
        assert_eq!(
            failure.category,
            FailureKind::Transient {
                retry_after_seconds: None
            },
            "{unreadable} must be retryable, not an account fault"
        );
        assert_eq!(
            failure.code.as_deref(),
            Some("filejoker.session_unconfirmed"),
            "{unreadable}"
        );
    }
}

#[tokio::test]
async fn check_account_reports_account_invalid_for_401() {
    let response = status_only("https://filejoker.net/", 401);
    let resolver = FilejokerResolver::new(MockHost::one_in_session(response));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("401 must fail");
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("filejoker.http_error"));
}

#[tokio::test]
async fn check_account_reports_rate_limited_for_429() {
    let response = status_only("https://filejoker.net/", 429);
    let resolver = FilejokerResolver::new(MockHost::one_in_session(response));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("429 must fail");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: None
        }
    );
}

// --- trait defaults ------------------------------------------------------------------------

#[tokio::test]
async fn check_defaults_to_unsupported() {
    let host = MockHost::without_cookies();
    let resolver = FilejokerResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .check(rd_plugin_api::CheckRequest {
            urls: vec!["https://filejoker.net/abc123xyz".parse().expect("URL")],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect_err("unsupported");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(
        failure.code.as_deref(),
        Some(crate::messages::CHECK_UNSUPPORTED.0)
    );
    assert_eq!(failure.message, crate::messages::CHECK_UNSUPPORTED.1);
}

/// `hosters()` reports the hoster this account can download from, once.
///
/// The native build used to answer this from the trait's default, which derives the list from
/// the manifest's intake domains and therefore said `filejoker.net` *and* `www.filejoker.net`;
/// the component answered from `crate::HOSTERS`, which is the deliberate list and says it once.
/// Now that both builds run the same code, the deliberate list is the one that survives.
#[tokio::test]
async fn hosters_reports_the_hoster_once() {
    let host = MockHost::without_cookies();
    let resolver = FilejokerResolver::new(host as Arc<dyn ResolverHost>);
    let hosters = resolver.hosters(AccountId::new()).await.expect("hosters");
    assert_eq!(hosters, vec!["filejoker.net".to_owned()]);
}

#[path = "free_tests.rs"]
mod free_tests;

#[path = "resolve_tests.rs"]
mod resolve_tests;

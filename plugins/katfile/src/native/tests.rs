//! MockHost suite ported from `plugins/ddownload/src/native/tests.rs` (validates the `xfs-common`
//! extraction: identical harness, KatFile fixtures/domains) plus KatFile-specific coverage the
//! brief calls out explicitly: more `check_account` variants, the captcha short-circuit (and,
//! after review, its form-scoping), an offline `file/info` mapping, `check()`'s batch mapping,
//! "zero requests when credentials are missing" gate tests for both `resolve()` and
//! `check_account()`, every alias domain being claimed and rewritten to the primary domain before
//! fetching, and the premium-only/pre-download-wait checks.
//!
//! Fixtures deliberately mix domains: `resolve_request()`'s input link uses the `katfile.com`
//! alias throughout (proving `canonicalize_host` normalizes it), while every mocked *response*
//! (`final_url`, download hrefs, API request-URL assertions) uses `katfile.biz` — the domain a
//! real server would actually respond from/serve links under once the request itself has been
//! rewritten to the primary domain.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};

use url::Url;

use super::KatfileResolver;

pub(crate) struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    pub(crate) requests: Mutex<Vec<HostHttpRequest>>,
    has_secret: bool,
    has_cookies: bool,
    /// Free-flow observations: the countdowns waited out and the challenges handed over, in
    /// the order the resolver produced them.
    pub(crate) waits: Mutex<Vec<u32>>,
    pub(crate) captchas: Mutex<Vec<rd_plugin_api::CaptchaChallenge>>,
    /// Token every captcha is answered with; `None` mimics a host with no solver.
    captcha_token: Option<String>,
}

impl MockHost {
    pub(crate) fn new(response: HostHttpResponse, has_secret: bool) -> Arc<Self> {
        Self::with_responses(vec![response], has_secret)
    }

    pub(crate) fn with_responses(responses: Vec<HostHttpResponse>, has_secret: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret,
            has_cookies: true,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: None,
        })
    }

    pub(crate) fn bare(has_secret: bool, has_cookies: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            has_secret,
            has_cookies,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: None,
        })
    }

    /// Host for the account-less free flow: no credentials at all, and every captcha
    /// answered with `captcha_token` (`None` mimics an instance with no solver configured).
    pub(crate) fn free(responses: Vec<HostHttpResponse>, captcha_token: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret: false,
            has_cookies: false,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: captcha_token.map(str::to_owned),
        })
    }

    /// Full constructor for a test that needs a queued response *and* no cookies (`with_responses`
    /// always sets `has_cookies: true`).
    pub(crate) fn full(
        responses: Vec<HostHttpResponse>,
        has_secret: bool,
        has_cookies: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret,
            has_cookies,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: None,
        })
    }
}

pub(crate) fn html(body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: "https://katfile.biz/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

pub(crate) fn file(final_url: &str) -> HostHttpResponse {
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

pub(crate) fn json(url: &str, body: &'static [u8]) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: url.parse().expect("URL"),
        headers: Vec::new(),
        body: body.to_vec(),
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

/// The site's own answer to a session probe, in the three shapes it can take. The sign-out
/// link is the measured positive marker; the header's `/login` link is what a lapsed session is
/// served; an interstitial carries neither and settles nothing (RD-120-13).
pub(crate) const SIGNED_IN_PAGE: &str = r#"<title>KatFile</title><a href="/?op=logout">Logout</a>"#;
pub(crate) const EXPIRED_SESSION_PAGE: &str =
    r#"<title>KatFile</title><a class="nav-link" href="/login">Login</a>"#;
pub(crate) const UNREADABLE_PAGE: &str =
    r#"<title>Just a moment...</title><div id="cf-wrapper"></div>"#;

/// The probe the account check makes with the cookie session, answered as `body`.
fn session_page(body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: "https://katfile.biz/".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

pub(crate) const FORM_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_premium" value="">
</form>"#;

pub(crate) const CAPTCHA_FORM_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_premium" value="">
<div class="g-recaptcha" data-sitekey="6Lc-site-key"></div>
</form>"#;

/// A captcha widget elsewhere on the page (e.g. a site-wide login modal) — not inside the
/// `download2` form itself. Finding 3: `has_captcha_challenge` must be scoped to the form, so
/// this must NOT abort the resolve.
pub(crate) const CAPTCHA_OUTSIDE_FORM_PAGE: &str = r#"<div class="login-modal"><div class="g-recaptcha" data-sitekey="unrelated-site-key"></div></div>
<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_premium" value="">
</form>"#;

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.lock().expect("mock lock").push(request);
        self.responses
            .lock()
            .expect("mock lock")
            .pop_front()
            .ok_or_else(|| Failure::new(rd_core::FailureKind::Permanent, "missing mock response"))
    }

    async fn cookies_get(&self, _account_id: AccountId, _url: &Url) -> Vec<(String, String)> {
        if self.has_cookies {
            vec![("xfss".to_owned(), "session".to_owned())]
        } else {
            Vec::new()
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        self.has_secret
    }

    /// Records the countdown instead of sleeping, so the flow's timing is asserted without
    /// slowing the suite down.
    async fn wait(&self, _client: &ClientIdentity, seconds: u32) -> Result<(), Failure> {
        self.waits.lock().expect("mock lock").push(seconds);
        Ok(())
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        challenge: rd_plugin_api::CaptchaChallenge,
        _limit: std::time::Duration,
    ) -> Result<rd_plugin_api::CaptchaAnswer, Failure> {
        self.captchas.lock().expect("mock lock").push(challenge);
        match &self.captcha_token {
            Some(token) => Ok(rd_plugin_api::CaptchaAnswer::Token(token.clone())),
            None => Err(Failure::coded(
                rd_core::FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            )),
        }
    }
}

/// The input link uses the `katfile.com` alias — every test built on this proves
/// `canonicalize_host` rewrites it to `katfile.biz` before the plugin issues any request.
pub(crate) fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://katfile.com/abc123xyz/release.rar"
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
fn every_alias_domain_is_claimed_by_matches() {
    let resolver = KatfileResolver::new(MockHost::bare(false, false));
    for host in [
        "katfile.biz",
        "katfile.space",
        "katfile.ws",
        "katfile.vip",
        "katfile.online",
        "katfile.cloud",
        "katfile.com",
    ] {
        let url: Url = format!("https://{host}/abc123xyz").parse().expect("URL");
        assert!(resolver.matches(&url), "{host} should be claimed");
    }
    assert!(!resolver.matches(&"https://example.com/abc123xyz".parse().expect("URL")));
}

#[tokio::test]
async fn alias_domain_link_is_rewritten_to_the_primary_domain_before_fetching() {
    let host = MockHost::with_responses(
        vec![
            html(FORM_PAGE),
            file("https://fs7.katfile.biz/d/r4nd/release.rar"),
        ],
        false,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    // resolve_request()'s URL is the katfile.com alias (see its own doc comment).
    resolver.resolve(resolve_request()).await.expect("resolved");
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[0].url.host_str(),
        Some("katfile.biz"),
        "the initial GET must target the primary domain, not the katfile.com alias the link used"
    );
}

// --- check_account -----------------------------------------------------------------------

#[tokio::test]
async fn account_info_error_reports_provider_message() {
    let response = HostHttpResponse {
        status: 200,
        final_url: "https://katfile.biz/api/account/info".parse().expect("URL"),
        headers: Vec::new(),
        body: br#"{"status":400,"server_time":"2026-08-30 13:43:41","msg":"Invalid key"}"#.to_vec(),
    };
    let resolver = KatfileResolver::new(MockHost::new(response, true));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("invalid key must fail");
    assert!(
        failure.message.contains("Invalid key"),
        "unexpected message: {}",
        failure.message
    );
    assert_eq!(failure.code.as_deref(), Some("katfile.api_error"));
}

#[tokio::test]
async fn check_account_reports_premium_with_traffic_and_the_exact_request() {
    let response = json(
        "https://katfile.biz/api/account/info",
        br#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2028-01-01 00:00:00","traffic_left":"1048576"}}"#,
    );
    let host = MockHost::with_responses(vec![response, session_page(SIGNED_IN_PAGE)], true);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("premium account");
    assert!(status.premium);
    assert!(
        plugin_common::native::label_summary(&status.label)
            .contains("plugin.account.user(user=user@example.test)")
    );
    // The XFS endpoint counts in megabytes and `AccountStatus` carries bytes. Asserting only
    // `is_some()` is what let this go unnoticed until an account with 112 GiB left was shown
    // as "112 KiB", so the expected value is spelled out.
    assert_eq!(
        status.traffic_left.map(rd_core::ByteCount::get),
        Some(1_048_576 * 1024 * 1024)
    );

    let requests = host.requests.lock().expect("mock lock");
    // Two requests, not one: the key answers for the account, and the cookie session the
    // download will run on is verified with a request of its own (RD-120-13).
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].url.as_str(),
        "https://katfile.biz/api/account/info",
        "API calls must target the primary domain"
    );
    assert!(
        requests[0]
            .query
            .iter()
            .any(|q| q.name == "key" && q.value_template == "{{secret:katfile_api_key}}")
    );
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].url.as_str(), "https://katfile.biz/");
    assert!(
        plugin_common::native::label_summary(&status.label)
            .contains("plugin.account.session_active()"),
        "a green check says the session was verified: {}",
        plugin_common::native::label_summary(&status.label)
    );
}

/// An expiry that has passed is not premium.
///
/// This used to assert the opposite, and said so: the decision was deliberately clock-free
/// because the WebAssembly guest had no clock to compare against, so both builds agreed by
/// agreeing on the wrong answer. The host now offers `now-unix-seconds`, and an account whose
/// premium ran out last week reports what it is.
#[tokio::test]
async fn check_account_reports_not_premium_for_an_expiry_that_has_passed() {
    let response = json(
        "https://katfile.biz/api/account/info",
        br#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2000-01-01 00:00:00","traffic_left":null}}"#,
    );
    let resolver = KatfileResolver::new(MockHost::with_responses(
        vec![response, session_page(SIGNED_IN_PAGE)],
        true,
    ));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("account still resolves");
    assert!(!status.premium);
}

/// An expiry still ahead is premium, which is the other half of the same decision.
#[tokio::test]
async fn check_account_reports_premium_while_the_expiry_is_still_ahead() {
    let response = json(
        "https://katfile.biz/api/account/info",
        br#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2099-01-01 00:00:00","traffic_left":null}}"#,
    );
    let resolver = KatfileResolver::new(MockHost::with_responses(
        vec![response, session_page(SIGNED_IN_PAGE)],
        true,
    ));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("account still resolves");
    assert!(status.premium);
}

#[tokio::test]
async fn check_account_reports_not_premium_for_an_empty_expiry() {
    let response = json(
        "https://katfile.biz/api/account/info",
        br#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"","traffic_left":null}}"#,
    );
    let resolver = KatfileResolver::new(MockHost::with_responses(
        vec![response, session_page(SIGNED_IN_PAGE)],
        true,
    ));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("account still resolves");
    assert!(!status.premium);
}

#[tokio::test]
async fn check_account_reports_account_invalid_for_401() {
    let response = status_only("https://katfile.biz/api/account/info", 401);
    let resolver = KatfileResolver::new(MockHost::new(response, true));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("401 must fail");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("katfile.http_error"));
}

#[tokio::test]
async fn check_account_reports_rate_limited_for_429() {
    let response = status_only("https://katfile.biz/api/account/info", 429);
    let resolver = KatfileResolver::new(MockHost::new(response, true));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("429 must fail");
    assert_eq!(
        failure.category,
        rd_core::FailureKind::RateLimited {
            retry_after_seconds: None
        }
    );
}

/// No API key, but a cookie session is present: `check_account` must issue a live GET against
/// the primary domain and report the account valid only once that GET actually succeeds
/// (Finding 2 of the pre-merge review: `guest.rs` used to skip this request entirely and report
/// `valid: true, premium: true` unconditionally).
#[tokio::test]
async fn check_account_with_cookies_but_no_api_key_probes_the_primary_domain() {
    let host = MockHost::new(session_page(SIGNED_IN_PAGE), false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("cookie-only account resolves");
    assert!(status.valid);

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 1, "must probe the primary domain live");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.as_str(), "https://katfile.biz/");
}

/// The reachable cookie session proves the cookies, not what was paid for: `premium_expire`
/// lives behind `api/account/info`, which knows only API keys, and this branch has none. Until
/// RD-109-38 it answered `premium: true` anyway and the interface printed "Premium active" for
/// a free account. The counter-proof is two tests up: a *read* expiry still reports premium.
#[tokio::test]
async fn a_cookie_only_session_does_not_claim_premium() {
    let resolver = KatfileResolver::new(MockHost::new(session_page(SIGNED_IN_PAGE), false));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("cookie-only account resolves");
    assert!(status.valid);
    assert!(!status.premium, "nothing here read the subscription");
    assert!(
        plugin_common::native::label_summary(&status.label)
            .contains("plugin.account.premium_unchecked()"),
        "the label must say what was not read: {}",
        plugin_common::native::label_summary(&status.label)
    );
}

/// The same live GET failing (e.g. the cookie session is stale and the server answers with a
/// non-2xx status) must fail `check_account`, not silently report a valid premium account.
#[tokio::test]
async fn check_account_with_cookies_but_no_api_key_fails_when_the_probe_fails() {
    let response = status_only("https://katfile.biz/", 403);
    let resolver = KatfileResolver::new(MockHost::new(response, false));
    resolver
        .check_account(AccountId::new())
        .await
        .expect_err("a failed live probe must not report a valid account");
}

#[tokio::test]
async fn check_account_without_secret_or_cookies_makes_no_requests() {
    let host = MockHost::bare(false, false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("missing credentials must fail");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.cookie_session_required")
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 0);
}

#[path = "session_tests.rs"]
mod session_tests;

#[path = "check_tests.rs"]
mod check_tests;

#[path = "free_tests.rs"]
mod free_tests;

#[path = "resolve_tests.rs"]
mod resolve_tests;

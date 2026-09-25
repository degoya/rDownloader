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

use super::DdownloadResolver;

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    /// Whether the API key slot answers. The real host admits exactly one slot per account,
    /// so a mock that answered every reference alike would let a test pass a combination the
    /// host can never produce.
    has_secret: bool,
    /// Whether the password slot answers, i.e. whether the account is in `login` mode.
    has_password: bool,
    has_cookies: bool,
    /// Free-flow observations: the countdowns waited out and the challenges handed over, in the
    /// order the resolver produced them.
    waits: Mutex<Vec<u32>>,
    captchas: Mutex<Vec<rd_plugin_api::CaptchaChallenge>>,
    /// Token every captcha is answered with; `None` mimics a host with no solver.
    captcha_token: Option<String>,
}

impl MockHost {
    fn new(response: HostHttpResponse, has_secret: bool) -> Arc<Self> {
        Self::with_responses(vec![response], has_secret)
    }

    fn with_responses(responses: Vec<HostHttpResponse>, has_secret: bool) -> Arc<Self> {
        Self::full(responses, has_secret, true)
    }

    /// Full constructor for a test that needs a queued response *and* no cookies
    /// (`with_responses` always sets `has_cookies: true`).
    fn full(responses: Vec<HostHttpResponse>, has_secret: bool, has_cookies: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret,
            has_password: false,
            has_cookies,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: None,
        })
    }

    /// Host for an account in `login` credential mode: the password slot answers, the API key
    /// slot does not, and there are no imported cookies — exactly what the real host reports
    /// for such an account.
    fn signing_in(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret: false,
            has_password: true,
            has_cookies: false,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: None,
        })
    }

    /// Host for a `login`-mode account whose session is already established: the password slot
    /// answers and the host's cookie jar holds the session an earlier sign-in produced. That is
    /// the state the real host is in for every call after the first one, and the state the
    /// account check used to fail in (RD-109-34).
    fn signed_in_session(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret: false,
            has_password: true,
            has_cookies: true,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: None,
        })
    }

    /// A `login`-mode account with a cold jar on an instance that can answer the login form's
    /// captcha widget - what the real login page has carried since 2026-09-17.
    fn signing_in_with_solver(responses: Vec<HostHttpResponse>, captcha_token: &str) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret: false,
            has_password: true,
            has_cookies: false,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: Some(captcha_token.to_owned()),
        })
    }

    /// Host for a cookie-only premium account whose instance has a solver: every captcha is
    /// answered with `captcha_token`.
    fn cookie_session_with_solver(
        responses: Vec<HostHttpResponse>,
        captcha_token: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret: false,
            has_password: false,
            has_cookies: true,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: Some(captcha_token.to_owned()),
        })
    }

    /// Host for the account-less free flow: no credentials at all, and every captcha answered
    /// with `captcha_token` (`None` mimics an instance with no solver configured).
    fn free(responses: Vec<HostHttpResponse>, captcha_token: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret: false,
            has_password: false,
            has_cookies: false,
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: captcha_token.map(str::to_owned),
        })
    }
}

fn html(body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: "https://ddownload.com/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn json(body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: "https://api-v2.ddownload.com/api/account/info"
            .parse()
            .expect("URL"),
        headers: Vec::new(),
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

const FORM_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_premium" value="">
</form>"#;

/// The file page as DDownload serves it since 2026-09-17, fetched without a session: the
/// `download2` form directly on the page, a Turnstile widget inside it, the countdown, and the
/// header's login link — which is on every page a guest sees (RD-108-28).
const FILE_PAGE_2026_09_17: &str = include_str!("../../tests/fixtures/file-page-2026-09-17.html");

/// DDownload's login page as it was served to a visitor without a session on 2026-09-20, the
/// day the account check was reported for failing on a session that was downloading files
/// (RD-109-34). It is also the page a cold cookie jar is redirected to from `/?op=my_account`.
const LOGIN_PAGE_2026_09_20: &str = include_str!("../../tests/fixtures/login-page-2026-09-20.html");

/// The same header over an XFS `class="err"` message and no form at all.
const ERROR_PAGE_2026_09_17: &str = include_str!("../../tests/fixtures/error-page-2026-09-17.html");

/// The measured file page with the download form cut out: the header with its login link, the
/// title, and nothing to post — the case that used to be reported as a login wall.
fn file_page_without_the_form() -> String {
    let start = FILE_PAGE_2026_09_17
        .find("<form name=\"F1\"")
        .expect("the fixture carries the form");
    let end = FILE_PAGE_2026_09_17
        .find("</form>")
        .expect("the form closes")
        + "</form>".len();
    format!(
        "{}{}",
        &FILE_PAGE_2026_09_17[..start],
        &FILE_PAGE_2026_09_17[end..]
    )
}

const TURNSTILE_SITE_KEY: &str = "0x4AAAAAABm53D0OJNkESa1O";

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

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        match reference {
            "ddownload_password" => self.has_password,
            _ => self.has_secret,
        }
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

#[tokio::test]
async fn api_error_without_result_reports_provider_message() {
    let response = HostHttpResponse {
        status: 200,
        final_url: "https://api-v2.ddownload.com/api/account/info"
            .parse()
            .expect("URL"),
        headers: Vec::new(),
        body: br#"{"status":400,"server_time":"2026-08-30 13:43:41","msg":"Invalid key"}"#.to_vec(),
    };
    let resolver = DdownloadResolver::new(MockHost::new(response, true));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("invalid key must fail");
    assert!(
        failure.message.contains("Invalid key"),
        "unexpected message: {}",
        failure.message
    );
}

#[tokio::test]
async fn remaining_traffic_is_reported_in_bytes() {
    // The XFS account endpoint counts in megabytes; `AccountStatus` carries bytes. Without the
    // conversion an account with 112 GiB left was shown as "112 KiB", because 114688 megabytes
    // were handed on as 114688 bytes.
    let response = HostHttpResponse {
        status: 200,
        final_url: "https://api-v2.ddownload.com/api/account/info"
            .parse()
            .expect("URL"),
        headers: Vec::new(),
        body: br#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2028-01-01 00:00:00","traffic_left":"114688"}}"#.to_vec(),
    };
    // The second answer is the session probe `check_account` now makes with the cookie jar:
    // the key answers for the account, and the download runs on the session (RD-120-13).
    let resolver = DdownloadResolver::new(MockHost::with_responses(
        vec![
            response,
            html(r#"<title>DDownload</title><a href="/?op=logout">Logout</a>"#),
        ],
        true,
    ));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("account status");
    assert_eq!(
        status.traffic_left.map(rd_core::ByteCount::get),
        Some(114_688 * 1024 * 1024),
        "112 GiB, not 112 KiB"
    );
}

#[tokio::test]
async fn cookie_probe_returns_final_transfer_url() {
    let response = HostHttpResponse {
        status: 206,
        final_url: "https://cdn.ddownload.com/file.bin".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Disposition".to_owned(),
            value: "attachment; filename=release.bin".to_owned(),
        }],
        body: vec![0],
    };
    let resolver = DdownloadResolver::new(MockHost::new(response, false));
    let account = AccountId::new();
    let resolved = resolver
        .resolve(ResolveRequest {
            url: "https://ddownload.com/abc123xyz".parse().expect("URL"),
            client: ClientIdentity {
                account_id: Some(account),
                proxy_profile_id: None,
                tls_revision: 4,
            },
        })
        .await
        .expect("resolved");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.client.account_id, Some(account));
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://ddownload.com/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

#[tokio::test]
async fn download_form_is_posted_and_redirect_target_is_used() {
    let host = MockHost::with_responses(
        vec![
            html(FORM_PAGE),
            file("https://fs7.ddownload.com/d/r4nd/release.rar"),
        ],
        false,
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.ddownload.com/d/r4nd/release.rar"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].body,
        b"op=download2&id=abc123xyz&rand=r4nd&method_premium=Premium+Download"
    );
    assert!(requests[1].headers.iter().any(|header| {
        header.name == "Content-Type"
            && header.value_template == "application/x-www-form-urlencoded"
    }));
}

#[tokio::test]
async fn direct_link_page_after_post_is_followed() {
    let host = MockHost::with_responses(
        vec![
            html(FORM_PAGE),
            html(
                r#"<a href="https://ddownload.com/premium">Premium</a><a href="https://fs7.ddownload.com/d/r4nd/release.rar">Download</a>"#,
            ),
            file("https://fs7.ddownload.com/d/r4nd/release.rar"),
        ],
        false,
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver.resolve(resolve_request()).await.expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.ddownload.com/d/r4nd/release.rar"
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 3);
}

#[tokio::test]
async fn guest_page_after_post_reports_missing_premium_session() {
    let host = MockHost::with_responses(
        vec![html(FORM_PAGE), html("<html>please wait 60 seconds</html>")],
        false,
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("guest session");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(failure.code.as_deref(), Some("ddownload.no_premium_file"));
    assert!(
        failure
            .message
            .starts_with("DDownload cookie session did not return a premium file:"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn api_key_without_cookie_session_cannot_download() {
    let host = MockHost::full(Vec::new(), true, false);
    let resolver = DdownloadResolver::new(host as Arc<dyn ResolverHost>);
    let failure = resolver
        .resolve(resolve_request())
        .await
        .expect_err("cookies required");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.cookie_session_required_for_download")
    );
    assert!(
        failure.message.contains("cookie session"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn api_direct_link_is_used_without_cookies() {
    let host = MockHost::full(
        vec![HostHttpResponse {
            status: 200,
            final_url: "https://api-v2.ddownload.com/api/file/direct_link"
                .parse()
                .expect("URL"),
            headers: Vec::new(),
            body: br#"{"status":200,"msg":"OK","result":{"url":"https://fs9.ddownload.com/d/tok/release.rar","size":"4096"}}"#.to_vec(),
        }],
        true,
        false,
    );
    let resolver = DdownloadResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let resolved = resolver
        .resolve(resolve_request())
        .await
        .expect("resolved via API");
    assert_eq!(
        resolved.url.as_str(),
        "https://fs9.ddownload.com/d/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

#[path = "session_tests.rs"]
mod session_tests;

#[path = "free_tests.rs"]
mod free_tests;

#[path = "premium_tests.rs"]
mod premium_tests;

#[path = "login_tests.rs"]
mod login_tests;

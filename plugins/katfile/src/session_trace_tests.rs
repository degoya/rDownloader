//! The trace of an unrecognized session page, and the canary that proves its body stays out.
//!
//! Driven against a mock [`PluginHost`] rather than through the `Resolver` trait, because the
//! question is what the plugin told the *host*: the native adapter forwards `log` straight into
//! `tracing`, where a test would have to install a subscriber to see anything at all.
//!
//! The canary page is not a recording. No KatFile page has been measured at all, signed in or
//! not; this one carries a made-up title and, around it, the kinds of value an account page
//! does carry — text, an address, the API key in a form field — each stamped with [`CANARY`],
//! so a line that leaks any of them is visible.

use std::{cell::RefCell, collections::VecDeque};

use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, FailureKind, HttpRequest,
    HttpResponse, PluginHost,
};

use super::unconfirmed_page_line;
use crate::resolver::{self, api};

const CANARY: &str = "c4n4ry7f3a";

/// A title over a body that settles nothing and carries three canaries.
const CANARY_PAGE: &str = r#"<html><head><title>KatFile - Cloud Storage</title></head>
<body><h1>Welcome back</h1><p>Balance for c4n4ry7f3a-account</p>
<span class="mail">c4n4ry7f3a@example.test</span>
<input type="text" name="api_key" value="c4n4ry7f3a-api-key-value" readonly>
<a href="/?op=my_account">My Account</a> <b>Premium</b></body></html>"#;

const ACCOUNT_INFO: &str = r#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2099-01-01 00:00:00","traffic_left":"204800"}}"#;

struct MockHost {
    answers: RefCell<VecDeque<HttpResponse>>,
    requests: RefCell<Vec<String>>,
    logs: RefCell<Vec<(String, String)>>,
    api_key: bool,
}

impl MockHost {
    fn new(answers: Vec<HttpResponse>, api_key: bool) -> Self {
        Self {
            answers: RefCell::new(answers.into()),
            requests: RefCell::new(Vec::new()),
            logs: RefCell::new(Vec::new()),
            api_key,
        }
    }
}

impl PluginHost for MockHost {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        self.requests.borrow_mut().push(request.url);
        self.answers.borrow_mut().pop_front().ok_or_else(|| {
            Failure::coded(
                FailureKind::Permanent,
                "mock.exhausted",
                "missing mock response",
            )
        })
    }

    async fn cookies(&self, _account_id: &str, _url: &str) -> Vec<(String, String)> {
        vec![("xfss".to_owned(), "session".to_owned())]
    }

    async fn secret_available(&self, _account_id: &str, reference: &str) -> bool {
        self.api_key && reference == api::API_KEY_REFERENCE
    }

    async fn random_bytes(&self, _count: u32) -> Vec<u8> {
        Vec::new()
    }

    async fn wait(&self, _seconds: u32) -> Result<(), Failure> {
        Ok(())
    }

    async fn solve_challenge(
        &self,
        _challenge: CaptchaChallenge,
    ) -> Result<CaptchaAnswer, Failure> {
        Err(Failure::coded(
            FailureKind::NeedsCaptcha,
            "captcha.no_solver",
            "No captcha solver is configured",
        ))
    }

    async fn solve_captcha(
        &self,
        _challenge: CaptchaChallenge,
    ) -> Result<CaptchaSolution, Failure> {
        Err(Failure::coded(
            FailureKind::NeedsCaptcha,
            "captcha.no_solver",
            "No captcha solver is configured",
        ))
    }

    async fn now_unix_seconds(&self) -> u64 {
        0
    }

    fn log(&self, level: &str, message: &str) {
        self.logs
            .borrow_mut()
            .push((level.to_owned(), message.to_owned()));
    }
}

fn response(final_url: &str, body: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        final_url: final_url.to_owned(),
        headers: vec![(
            "Content-Type".to_owned(),
            "text/html; charset=UTF-8".to_owned(),
        )],
        body: body.as_bytes().to_vec(),
    }
}

fn homepage(body: &str) -> HttpResponse {
    response("https://katfile.biz/", body)
}

/// The one warn line the check left, after asserting there is exactly one.
fn single_warn_line(host: &MockHost) -> String {
    let logs = host.logs.borrow();
    assert_eq!(logs.len(), 1, "exactly one line: {logs:?}");
    assert_eq!(logs[0].0, "warn");
    logs[0].1.clone()
}

fn assert_no_canary(line: &str) {
    assert!(
        !line.to_ascii_lowercase().contains(CANARY),
        "the page body must not reach the log: {line}"
    );
}

#[test]
fn the_line_names_title_length_and_markers_and_nothing_else() {
    let line = unconfirmed_page_line("KatFile", "the homepage", CANARY_PAGE);
    assert_no_canary(&line);
    assert!(
        line.starts_with("KatFile: the homepage settled nothing"),
        "{line}"
    );
    assert!(line.contains("title \"KatFile - Cloud Storage\""), "{line}");
    assert!(
        line.contains(&format!("{} bytes", CANARY_PAGE.len())),
        "{line}"
    );
    assert!(
        line.ends_with("markers: op=my_account, my account, premium"),
        "{line}"
    );
}

#[test]
fn a_page_without_title_or_markers_says_so() {
    let line = unconfirmed_page_line("KatFile", "the homepage", "<p>c4n4ry7f3a</p>");
    assert_no_canary(&line);
    assert!(line.contains("no title"), "{line}");
    assert!(line.ends_with("markers: none"), "{line}");
}

#[test]
fn the_title_is_bounded_and_carries_no_control_characters() {
    let long = format!("<title>{}\u{7}\n tail</title>", "x".repeat(200));
    let line = unconfirmed_page_line("KatFile", "the homepage", &long);
    assert!(
        line.contains(&format!("title \"{}\"", "x".repeat(80))),
        "{line}"
    );
    assert!(!line.chars().any(char::is_control), "{line:?}");
}

/// The cookie-only branch: nothing but the session proves the account, so an unrecognized
/// page is reported as unconfirmed — retryable, not invalid, not a pass — and traced.
#[tokio::test]
async fn a_cookie_only_check_traces_the_unconfirmed_page_without_its_body() {
    let host = MockHost::new(vec![homepage(CANARY_PAGE)], false);
    let failure = resolver::check_account(&host, "account")
        .await
        .expect_err("an unrecognized page is no proof of a cookie-only account");
    assert_eq!(failure.kind, FailureKind::Transient(None));
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.cookie_session_unconfirmed")
    );
    assert_eq!(
        host.requests.borrow().as_slice(),
        ["https://katfile.biz/"],
        "the homepage: no KatFile page is measured, so none replaces it"
    );
    let line = single_warn_line(&host);
    assert_no_canary(&line);
    assert!(line.contains("KatFile - Cloud Storage"), "{line}");
}

/// The `api_key` branch: the key proved the account, the check passes, and the page that
/// settled nothing is traced the same way.
#[tokio::test]
async fn a_proven_key_traces_the_unconfirmed_page_without_its_body() {
    let host = MockHost::new(
        vec![
            response("https://katfile.biz/api/account/info", ACCOUNT_INFO),
            homepage(CANARY_PAGE),
        ],
        true,
    );
    let account = resolver::check_account(&host, "account")
        .await
        .expect("an unconfirmed session does not fail an account the key has proven");
    assert!(account.valid);
    assert!(account.premium, "premium comes from the API");
    let codes: Vec<&str> = account
        .label
        .iter()
        .map(|part| part.code.as_str())
        .collect();
    assert!(codes.contains(&"katfile.session_unconfirmed"), "{codes:?}");
    assert!(
        !codes.contains(&"plugin.account.session_active"),
        "nothing confirmed the session, so nothing claims it: {codes:?}"
    );
    let line = single_warn_line(&host);
    assert_no_canary(&line);
    assert!(line.contains("markers: op=my_account"), "{line}");
}

/// A page that settles the question has nothing to trace.
#[tokio::test]
async fn a_recognized_page_writes_nothing() {
    let host = MockHost::new(
        vec![homepage(
            r#"<title>KatFile</title><a href="/?op=logout">Logout</a>"#,
        )],
        false,
    );
    resolver::check_account(&host, "account")
        .await
        .expect("a signed-in page is a session");
    assert!(host.logs.borrow().is_empty());
}

//! A mock host and the cases the XFS plugins' suites share (RD-1120-10, PL-2).
//!
//! `ddownload` and `katfile` carried the same direct-link suite, and with `filejoker` the same
//! session-trace suite, each with its own copy of one mock host, different in nothing but the
//! provider's name, its pages' addresses and its codes. The cases are here once, taking those as
//! parameters; every plugin keeps its own test functions, each a call into a case with its own
//! values, so a plugin's suite still says what it covers and still fails under its own name.
//!
//! The host answers from a queue and records what it was asked and what it was told to log. It
//! implements [`PluginHost`] rather than the native `ResolverHost`, because these cases ask what
//! the plugin told the *host*: the native adapter forwards `log` straight into `tracing`, where
//! a test would have to install a subscriber to see anything at all.
//!
//! Behind the `test-support` feature, which only the plugins' dev-dependencies switch on.

use std::{cell::RefCell, collections::VecDeque};

use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, FailureKind, HttpRequest,
    HttpResponse, PluginHost,
};

mod direct_link;
mod session_trace;

pub use direct_link::{CODE, DirectLinkCase};
pub use session_trace::{TraceCase, assert_nothing_written};

/// A host that answers from a queue and records requests and log lines.
pub struct LogHost {
    answers: RefCell<VecDeque<Result<HttpResponse, Failure>>>,
    requests: RefCell<Vec<String>>,
    logs: RefCell<Vec<(String, String)>>,
    cookies: Vec<(String, String)>,
    secret: Option<&'static str>,
}

impl LogHost {
    /// A host handing out `answers` in order, with no cookies and no credential.
    #[must_use]
    pub fn new(answers: Vec<Result<HttpResponse, Failure>>) -> Self {
        Self {
            answers: RefCell::new(answers.into()),
            requests: RefCell::new(Vec::new()),
            logs: RefCell::new(Vec::new()),
            cookies: Vec::new(),
            secret: None,
        }
    }

    /// The same host with an XFS session cookie in every account's jar.
    #[must_use]
    pub fn with_session(mut self) -> Self {
        self.cookies = vec![("xfss".to_owned(), "session".to_owned())];
        self
    }

    /// The same host with the account holding a credential under `reference`, and only there.
    #[must_use]
    pub fn with_secret(mut self, reference: &'static str) -> Self {
        self.secret = Some(reference);
        self
    }

    /// The URLs requested so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<String> {
        self.requests.borrow().clone()
    }

    /// The `(level, message)` lines logged so far, in order.
    #[must_use]
    pub fn logs(&self) -> Vec<(String, String)> {
        self.logs.borrow().clone()
    }
}

fn no_solver() -> Failure {
    Failure::coded(
        FailureKind::NeedsCaptcha,
        "captcha.no_solver",
        "No captcha solver is configured",
    )
}

impl PluginHost for LogHost {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        self.requests.borrow_mut().push(request.url);
        self.answers.borrow_mut().pop_front().unwrap_or_else(|| {
            Err(Failure::coded(
                FailureKind::Permanent,
                "mock.exhausted",
                "missing mock response",
            ))
        })
    }

    async fn cookies(&self, _account_id: &str, _url: &str) -> Vec<(String, String)> {
        self.cookies.clone()
    }

    async fn secret_available(&self, _account_id: &str, reference: &str) -> bool {
        self.secret == Some(reference)
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
        Err(no_solver())
    }

    async fn solve_captcha(
        &self,
        _challenge: CaptchaChallenge,
    ) -> Result<CaptchaSolution, Failure> {
        Err(no_solver())
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

/// A `200` answer from `final_url` carrying `body`, without headers: what a JSON API returns.
#[must_use]
pub fn json(final_url: &str, body: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        final_url: final_url.to_owned(),
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// A `200` HTML page from `final_url`.
#[must_use]
pub fn html(final_url: &str, body: &str) -> HttpResponse {
    HttpResponse {
        headers: vec![(
            "Content-Type".to_owned(),
            "text/html; charset=UTF-8".to_owned(),
        )],
        ..json(final_url, body)
    }
}

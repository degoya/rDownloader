//! A scripted [`ResolverHost`] for the native plugin suites (RD-1120-10, PL-3).
//!
//! Nineteen plugins drove their resolver against the same mock, each a copy: a queue of
//! answers handed out in order, every request recorded, a switch for whether the account's
//! credential exists. This is that mock once. It answers at the host boundary, so no socket is
//! opened and a test sees each request exactly as the plugin described it — templates such as
//! `{{secret:…}}` included, never a value.
//!
//! Countdowns are recorded instead of slept, and every captcha is recorded and answered with
//! the configured token, or refused as a host without a solver refuses. What a plugin's own
//! suite needs beyond that — an XFS cookie session, a second credential slot — it builds from
//! [`ScriptedHost::scripted`] in a few lines of its own.
//!
//! Behind the `test-support` feature, which only the plugins' dev-dependencies switch on: the
//! service never carries it.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use url::Url;

use crate::{
    CaptchaAnswer, CaptchaChallenge, ClientIdentity, HostHttpRequest, HostHttpResponse,
    ResolverHost,
};

/// Decides, per vault reference, whether the account holds a credential there.
type SecretRule = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// A host that answers from a script and records what the plugin asked of it.
///
/// The observations are public fields so an assertion reads them in place; they are only ever
/// appended to, in the order the plugin produced them.
pub struct ScriptedHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    /// Every request the plugin sent, answered or not.
    pub requests: Mutex<Vec<HostHttpRequest>>,
    /// The countdowns the plugin waited out, in seconds.
    pub waits: Mutex<Vec<u32>>,
    /// The challenges the plugin handed over.
    pub captchas: Mutex<Vec<CaptchaChallenge>>,
    /// `"captcha"` and `"wait"` in the order they happened, so a test can assert that a token
    /// was minted before its countdown ran out.
    pub order: Mutex<Vec<&'static str>>,
    /// Set once the plugin asked for cookies at all — a flow that must work without an
    /// account may never ask.
    pub cookies_asked: Mutex<bool>,
    secret: SecretRule,
    cookies: Vec<(String, String)>,
    captcha_token: Option<String>,
}

impl ScriptedHost {
    /// A host answering `responses` in order, with no credential, no cookie and no captcha
    /// solver. The starting point for a host a suite shapes further; see [`Self::shared`].
    #[must_use]
    pub fn scripted(responses: Vec<HostHttpResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            order: Mutex::new(Vec::new()),
            cookies_asked: Mutex::new(false),
            secret: Box::new(|_: &str| false),
            cookies: Vec::new(),
            captcha_token: None,
        }
    }

    /// Every reference answers alike: the account holds a credential, or it holds none.
    #[must_use]
    pub fn secret(mut self, available: bool) -> Self {
        self.secret = Box::new(move |_: &str| available);
        self
    }

    /// Only `reference` can answer, and only when `available`. The real host admits one slot
    /// per name, so a mock that answered every reference alike would let a test pass a
    /// combination the host can never produce.
    #[must_use]
    pub fn secret_for(mut self, reference: &'static str, available: bool) -> Self {
        self.secret = Box::new(move |asked: &str| available && asked == reference);
        self
    }

    /// Any other mapping from reference to availability, e.g. two slots set independently.
    #[must_use]
    pub fn secret_rule(mut self, rule: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.secret = Box::new(rule);
        self
    }

    /// The cookies the host's jar returns for the account, whatever the URL.
    #[must_use]
    pub fn cookies(mut self, cookies: &[(&str, &str)]) -> Self {
        self.cookies = cookies
            .iter()
            .map(|&(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        self
    }

    /// The token every captcha is answered with; `None` is an instance with no solver.
    #[must_use]
    pub fn captcha_token(mut self, token: Option<&str>) -> Self {
        self.captcha_token = token.map(str::to_owned);
        self
    }

    /// The host as a resolver takes it.
    #[must_use]
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// One answer; the account's credential exists under every reference when `has_secret`.
    #[must_use]
    pub fn new(response: HostHttpResponse, has_secret: bool) -> Arc<Self> {
        Self::with_responses(vec![response], has_secret)
    }

    /// Answers in order; the account's credential exists under every reference when
    /// `has_secret`.
    #[must_use]
    pub fn with_responses(responses: Vec<HostHttpResponse>, has_secret: bool) -> Arc<Self> {
        Self::scripted(responses).secret(has_secret).shared()
    }

    /// An account-less guest: no credential, no cookie, no captcha solver.
    #[must_use]
    pub fn answering(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Self::scripted(responses).shared()
    }

    /// The account-less free flow: no credential, no cookie, and every captcha answered with
    /// `captcha_token` (`None` mimics an instance with no solver configured).
    #[must_use]
    pub fn free(responses: Vec<HostHttpResponse>, captcha_token: Option<&str>) -> Arc<Self> {
        Self::scripted(responses)
            .captcha_token(captcha_token)
            .shared()
    }

    /// A copy of every request sent so far.
    pub fn requests(&self) -> Vec<HostHttpRequest> {
        self.requests.lock().expect("mock lock").clone()
    }

    /// The request sent at `index`; panics when there is none, which fails the test.
    pub fn request(&self, index: usize) -> HostHttpRequest {
        self.requests.lock().expect("mock lock")[index].clone()
    }

    /// How many requests were sent.
    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("mock lock").len()
    }

    /// How many challenges were handed over.
    pub fn captcha_count(&self) -> usize {
        self.captchas.lock().expect("mock lock").len()
    }
}

#[async_trait]
impl ResolverHost for ScriptedHost {
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
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "missing mock response"))
    }

    async fn cookies_get(&self, _account_id: AccountId, _url: &Url) -> Vec<(String, String)> {
        *self.cookies_asked.lock().expect("mock lock") = true;
        self.cookies.clone()
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        (self.secret)(reference)
    }

    /// Records the countdown instead of sleeping, so a flow's timing is asserted without
    /// slowing the suite down.
    async fn wait(&self, _client: &ClientIdentity, seconds: u32) -> Result<(), Failure> {
        self.waits.lock().expect("mock lock").push(seconds);
        self.order.lock().expect("mock lock").push("wait");
        Ok(())
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        challenge: CaptchaChallenge,
        _time_limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        self.captchas.lock().expect("mock lock").push(challenge);
        self.order.lock().expect("mock lock").push("captcha");
        match &self.captcha_token {
            Some(token) => Ok(CaptchaAnswer::Token(token.clone())),
            None => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            )),
        }
    }
}

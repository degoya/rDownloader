//! What the executor needs from the outside, as four small traits.
//!
//! The crate stays a leaf (see `AGENTS.md` and RD-110-05, decision 3): it owns the three
//! bolts — host narrowing, redirect checking, the ban on private address ranges — and it owns
//! the steps, but it owns no HTTP client, no captcha broker and no clock it cannot be lied
//! to about. The caller supplies those; the tests supply recorded answers instead, which is
//! the only way a bolt that refuses `127.0.0.1` can be tested without a server on
//! `127.0.0.1`.
//!
//! **Redirects are reported, not followed.** A client that follows them on its own hides
//! every hop from the executor, and the hop is exactly what has to be checked. An adapter
//! therefore configures its client for no redirect policy and hands back the 3xx response.
//!
//! **The adapter connects to the addresses it is handed, and resolves nothing itself.** The
//! executor resolves the host once, through [`HostResolver`], and refuses every private,
//! loopback and link-local answer. If the adapter then let its own client resolve the name
//! again, a record with a time-to-live of zero that flips to `127.0.0.1` between the two
//! lookups would walk straight through the bolt — the check and the connection would be
//! about different addresses. [`FetchRequest::addresses`] closes that door, and honouring it
//! is part of the contract, not an optimisation: with `reqwest` it is
//! `ClientBuilder::resolve_to_addrs(host, &addresses)`.

use std::{collections::BTreeMap, net::IpAddr, time::Duration};

use async_trait::async_trait;
use url::Url;

/// The two methods a rule can cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    /// The name as HTTP spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// One outgoing request. The executor has already checked the address when this is built.
#[derive(Clone, Debug)]
pub struct FetchRequest {
    pub url: Url,
    /// The addresses the executor resolved `url`'s host to **and checked**. The adapter must
    /// connect to one of these and must not resolve the name again; see the module
    /// documentation for why. Empty exactly when the host is already a literal address, so
    /// there was nothing to resolve and nothing to pin.
    pub addresses: Vec<IpAddr>,
    pub method: Method,
    /// Form fields for a `POST`, sent as `application/x-www-form-urlencoded`. Empty for a
    /// `GET`.
    pub form: BTreeMap<String, String>,
    /// Most bytes the adapter may read of the body. Reading beyond this is pointless work
    /// and an adapter should stop there and answer [`FetchFailure::TooLarge`]; the executor
    /// checks the length it got back as well, so an adapter that ignores this is caught.
    pub max_bytes: usize,
    /// Longest one request may take, redirects excluded — each hop is its own request.
    pub timeout: Duration,
}

/// What came back. Headers carry lowercase names, as HTTP/2 and `reqwest` hand them over.
#[derive(Clone, Debug)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl FetchResponse {
    /// A 200 with a body and no headers, for an adapter or a test that has nothing else.
    #[must_use]
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// The first value of `name`, compared case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Why a request produced no response at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchFailure {
    /// No DNS entry, a refused connection, a reset: nothing answered.
    Unreachable(String),
    /// Something answered, too late.
    Timeout,
    /// The body passed [`FetchRequest::max_bytes`] while it was being read.
    TooLarge,
    /// Anything else the client reports: TLS, a malformed response, a broken proxy.
    Other(String),
}

/// One HTTP request, without following redirects, to an address the executor already checked.
///
/// An implementation has two obligations beyond making the request, and both are load-bearing
/// rather than advisory: it must **not** follow redirects, and it must connect to
/// [`FetchRequest::addresses`] rather than resolving the host itself.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure>;
}

/// Name to addresses. The ban on private ranges is checked against what this returns, never
/// against the name, so a name that resolves into the local network is refused rather than
/// trusted for looking public.
///
/// The executor calls this **once** per request and hands the result to the fetcher, so the
/// address that was checked is the address that is connected to.
#[async_trait]
pub trait HostResolver: Send + Sync {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String>;
}

/// The challenge a `captcha` step hands over.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptchaRequest {
    /// The kind, as the rule spells it: `recaptcha-v2`, `hcaptcha`, ... Which kinds can be
    /// answered is the broker's business and RD-110-15's.
    pub challenge: String,
    /// The site key the rule read out of the page, already expanded.
    pub sitekey: Option<String>,
    /// The page the challenge sits on; a widget captcha is bound to its host.
    pub page_url: Url,
}

/// The captcha broker, as the executor needs it: a challenge in, a token out.
#[async_trait]
pub trait CaptchaSolver: Send + Sync {
    async fn solve(&self, request: CaptchaRequest) -> Result<String, String>;
}

/// Monotonic time since the run's origin.
///
/// A port rather than [`std::time::Instant`] so the total-time limit has a test that proves
/// it without spending the budget it caps.
pub trait Clock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

/// The clock a running installation uses.
#[derive(Debug)]
pub struct SystemClock {
    start: std::time::Instant,
}

impl SystemClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

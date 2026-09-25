//! Recorded answers instead of a network, for the tests of this module.
//!
//! Every limit below is proven against these: a fetcher that answers from a table, a resolver
//! that can be told to point a perfectly ordinary name at `127.0.0.1`, and a clock the
//! fetcher charges so the time budget is spent without waiting for it.

use std::{
    collections::BTreeMap,
    net::IpAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;

use super::ports::{
    CaptchaRequest, CaptchaSolver, Clock, FetchFailure, FetchRequest, FetchResponse, Fetcher,
    HostResolver,
};

pub(crate) type Answer = Result<FetchResponse, FetchFailure>;

/// A fetcher that answers from a table of addresses.
#[derive(Default)]
pub(crate) struct Recorded {
    answers: BTreeMap<String, Answer>,
    fallback: Option<Answer>,
    requests: Mutex<Vec<FetchRequest>>,
    charges: Option<(Arc<TestClock>, Duration)>,
}

impl Recorded {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A 200 with `body`.
    pub(crate) fn page(mut self, url: &str, body: &str) -> Self {
        self.answers
            .insert(url.to_owned(), Ok(FetchResponse::ok(body)));
        self
    }

    /// A redirect answer, which the executor follows itself.
    pub(crate) fn redirect(mut self, url: &str, status: u16, location: &str) -> Self {
        self.answers.insert(
            url.to_owned(),
            Ok(FetchResponse {
                status,
                headers: vec![("location".to_owned(), location.to_owned())],
                body: String::new(),
            }),
        );
        self
    }

    /// Any other answer, including a failure.
    pub(crate) fn answer(mut self, url: &str, answer: Answer) -> Self {
        self.answers.insert(url.to_owned(), answer);
        self
    }

    /// What every address not in the table answers.
    pub(crate) fn everything_else(mut self, answer: Answer) -> Self {
        self.fallback = Some(answer);
        self
    }

    /// Makes every request cost `cost` on `clock`, so the time budget can be spent in a test
    /// that finishes at once.
    pub(crate) fn charging(mut self, clock: &Arc<TestClock>, cost: Duration) -> Self {
        self.charges = Some((Arc::clone(clock), cost));
        self
    }

    /// The requests made, in order.
    pub(crate) fn requests(&self) -> Vec<FetchRequest> {
        self.requests
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl Fetcher for Recorded {
    async fn fetch(&self, request: FetchRequest) -> Answer {
        if let Some((clock, cost)) = &self.charges {
            clock.advance(*cost);
        }
        let key = request.url.as_str().to_owned();
        if let Ok(mut seen) = self.requests.lock() {
            seen.push(request);
        }
        self.answers
            .get(&key)
            .or(self.fallback.as_ref())
            .cloned()
            .unwrap_or_else(|| Err(FetchFailure::Unreachable(format!("{key} was not recorded"))))
    }
}

/// A resolver with a table and a default.
pub(crate) struct Dns {
    map: BTreeMap<String, Vec<IpAddr>>,
    fallback: Vec<IpAddr>,
}

impl Dns {
    /// Every name answers with a routable address.
    pub(crate) fn public() -> Self {
        Self {
            map: BTreeMap::new(),
            fallback: vec![address("93.184.216.34")],
        }
    }

    /// Names one host's answer, leaving the rest public.
    pub(crate) fn pointing(mut self, host: &str, addresses: &[&str]) -> Self {
        self.map.insert(
            host.to_owned(),
            addresses.iter().copied().map(address).collect(),
        );
        self
    }

    /// A name that does not resolve at all.
    pub(crate) fn missing(mut self, host: &str) -> Self {
        self.map.insert(host.to_owned(), Vec::new());
        self
    }
}

fn address(text: &str) -> IpAddr {
    text.parse().expect("address")
}

#[async_trait]
impl HostResolver for Dns {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        Ok(self
            .map
            .get(host)
            .cloned()
            .unwrap_or_else(|| self.fallback.clone()))
    }
}

/// A resolver whose answer changes after the first call: the shape of a record with a
/// time-to-live of zero, which is what makes a second lookup inside the adapter dangerous.
#[derive(Default)]
pub(crate) struct FlippingDns {
    pub(crate) calls: AtomicU64,
}

#[async_trait]
impl HostResolver for FlippingDns {
    async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(vec![address("93.184.216.34")])
        } else {
            Ok(vec![address("127.0.0.1")])
        }
    }
}

/// A clock that only moves when something charges it.
#[derive(Default)]
pub(crate) struct TestClock(AtomicU64);

impl TestClock {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn elapsed(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }
}

/// A broker that always answers the same way.
pub(crate) struct Broker(pub(crate) Result<String, String>);

#[async_trait]
impl CaptchaSolver for Broker {
    async fn solve(&self, _request: CaptchaRequest) -> Result<String, String> {
        self.0.clone()
    }
}

/// A broker that records what it was asked.
#[derive(Default)]
pub(crate) struct RecordingBroker {
    pub(crate) seen: Mutex<Vec<CaptchaRequest>>,
}

#[async_trait]
impl CaptchaSolver for RecordingBroker {
    async fn solve(&self, request: CaptchaRequest) -> Result<String, String> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request);
        }
        Ok("token-42".to_owned())
    }
}

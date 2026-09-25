//! The rule executor (RD-110-05): a rule and an address in, the links behind that page out.
//!
//! **Native, not a Wasm guest.** A rule is a record — an address pattern, some regular
//! expressions and a list of steps — not somebody else's code, and the sandbox exists to
//! contain somebody else's code. Running the steps natively puts the three bolts in one
//! place instead of in every plugin, and a rule edited in the interface takes effect at once
//! rather than after a signed rebuild. The exception is for *data*; protectors keep their own
//! logic and stay Wasm (RD-110-16, RD-110-17).
//!
//! **No HTTP client, no captcha broker, no clock.** Those are [`ports`], supplied by the
//! caller, so this crate stays the leaf `AGENTS.md` describes and so every limit below has a
//! test that neither waits nor reaches the network. The adapters live with RD-110-06, where
//! the client already is.
//!
//! **No JavaScript interpreter, ever.** [`decode`] covers the five ways these pages encode an
//! address. What is genuinely a program stays undecoded and the run refuses with
//! `site_rules.decode_failed`.

pub mod error;
pub mod ports;
pub mod value;

mod decode;
mod guard;
mod run;
mod steps;

use std::time::Duration;

use url::Url;

pub use error::RunError;
pub use ports::{
    CaptchaRequest, CaptchaSolver, Clock, FetchFailure, FetchRequest, FetchResponse, Fetcher,
    HostResolver, Method, SystemClock,
};
pub use value::{Value, Variables};

use crate::format::Rule;
use run::Run;

/// Most links one run may hand back, the same number the plugin host allows a crawler
/// (`rd_plugin_host::extension::crawler::MAX_CRAWLED_LINKS`). A page that claims more is not
/// a release page, and the two numbers stay equal on purpose: a rule and a crawler plugin
/// feed the same queue.
pub const MAX_LINKS: usize = 1_000;

/// What one run may spend. Every field has its own refusal, so a caller reading the code
/// knows which budget ran out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// How many requests deep an address may sit from the one the run was given. Every
    /// request counts, redirect hops included.
    pub max_depth: u32,
    /// How many requests the whole run may make.
    pub max_pages: u32,
    /// How many links the rule may produce; see [`MAX_LINKS`].
    pub max_links: usize,
    /// How long the whole run may take, however few steps it has.
    pub max_total_time: Duration,
    /// How large one response body may be.
    pub max_response_bytes: usize,
    /// How long one request may take. Passed to the adapter; the run's own budget is
    /// [`Self::max_total_time`].
    pub request_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_pages: 24,
            max_links: MAX_LINKS,
            max_total_time: Duration::from_secs(90),
            max_response_bytes: 4 * 1024 * 1024,
            request_timeout: Duration::from_secs(20),
        }
    }
}

/// What a successful run produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Crawl {
    /// The address that was actually crawled: the one given, or the canonical host it was
    /// revived onto when it arrived on a dead domain.
    pub address: Url,
    /// The links, absolute and deduplicated, in the order the rule found them.
    pub links: Vec<String>,
    /// The package name, when the rule's `package` source found one. A hint, as
    /// `crawled-link.package-hint` is: a run that found links but no title still succeeds.
    pub package_name: Option<String>,
    /// How many requests it took, for the log.
    pub pages_fetched: u32,
    /// Whether the rule declared these links to be copies of one file (RD-110-18). Passed
    /// through from `Rule::mirrors`: the executor states what the rule says and decides
    /// nothing about it.
    pub mirrors: bool,
}

/// The four ports one run borrows, carried together so every signature below takes one
/// argument rather than four.
#[derive(Clone, Copy)]
pub(crate) struct Ports<'a> {
    pub(crate) fetcher: &'a dyn Fetcher,
    pub(crate) resolver: &'a dyn HostResolver,
    pub(crate) captcha: Option<&'a dyn CaptchaSolver>,
    pub(crate) clock: &'a dyn Clock,
}

/// Runs rules. Holds the ports and the limits; one instance serves any number of runs.
pub struct Executor<'a> {
    ports: Ports<'a>,
    limits: Limits,
}

impl<'a> Executor<'a> {
    /// An executor with the default limits and no captcha broker: a `captcha` step then
    /// refuses with `site_rules.captcha_failed` rather than being skipped.
    #[must_use]
    pub fn new(
        fetcher: &'a dyn Fetcher,
        resolver: &'a dyn HostResolver,
        clock: &'a dyn Clock,
    ) -> Self {
        Self {
            ports: Ports {
                fetcher,
                resolver,
                captcha: None,
                clock,
            },
            limits: Limits::default(),
        }
    }

    /// Gives the executor a captcha broker.
    #[must_use]
    pub fn with_captcha(mut self, captcha: &'a dyn CaptchaSolver) -> Self {
        self.ports.captcha = Some(captcha);
        self
    }

    /// Replaces the limits.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The limits this executor runs with.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Runs `rule` against `address`.
    ///
    /// An address on one of the rule's dead hosts is rewritten to the canonical host first,
    /// which is what makes a year-old bookmark work. An address the rule does not claim is
    /// refused with the one code that lets the selection keep looking.
    pub async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        let origin_host = address.host_str().unwrap_or_default().to_owned();
        let address = rule.revive(address).unwrap_or_else(|| address.clone());
        if !rule.claims(&address) {
            return Err(RunError::NotClaimed(address.to_string()));
        }
        let mut run = Run::new(self.ports, self.limits, rule, address, origin_host);
        for (index, step) in rule.steps.iter().enumerate() {
            run.step(index, step).await?;
        }
        run.check_time()?;
        Ok(Crawl {
            address: run.address.clone(),
            links: run.links()?,
            package_name: run.package_name(),
            pages_fetched: run.pages(),
            mirrors: rule.mirrors,
        })
    }
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod exec_tests;
#[cfg(test)]
pub(crate) mod fakes;
#[cfg(test)]
#[path = "limit_tests.rs"]
mod limit_tests;

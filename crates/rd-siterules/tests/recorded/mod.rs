//! The recorded-answer harness the rule tests share.
//!
//! Nothing here touches the network: a fetcher answers from a table, the resolver answers
//! every name with one routable address, and the rule under test is an example the app brings
//! (`resources/examples.json`, RD-1230-03) or a synthetic one beside its fixtures. A service that
//! goes down therefore breaks the self-test (RD-110-09) rather than the build.

use std::{collections::BTreeMap, net::IpAddr};

use async_trait::async_trait;
use rd_siterules::{
    Crawl, Executor, Rule, RunError, SystemClock,
    exec::ports::{FetchFailure, FetchRequest, FetchResponse, Fetcher, HostResolver},
};
use url::Url;

/// A fetcher that answers from a table, so a test spends no network.
#[derive(Default)]
pub struct Recorded(BTreeMap<String, FetchResponse>);

#[allow(
    dead_code,
    reason = "each test binary uses a different subset of the harness"
)]
impl Recorded {
    pub fn page(mut self, url: &str, body: &str) -> Self {
        self.0.insert(url.to_owned(), FetchResponse::ok(body));
        self
    }

    pub fn status(mut self, url: &str, status: u16) -> Self {
        self.0.insert(
            url.to_owned(),
            FetchResponse {
                status,
                headers: Vec::new(),
                body: String::new(),
            },
        );
        self
    }

    /// A 302 with a `location`, which is what the `redirect` step reads.
    pub fn redirect(mut self, url: &str, location: &str) -> Self {
        self.0.insert(
            url.to_owned(),
            FetchResponse {
                status: 302,
                headers: vec![("location".to_owned(), location.to_owned())],
                body: String::new(),
            },
        );
        self
    }
}

#[async_trait]
impl Fetcher for Recorded {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure> {
        self.0
            .get(request.url.as_str())
            .cloned()
            .ok_or_else(|| FetchFailure::Unreachable(format!("{} was not recorded", request.url)))
    }
}

/// Every name answers with one routable address; the bolts have their own tests.
pub struct PublicDns;

#[async_trait]
impl HostResolver for PublicDns {
    async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
        Ok(vec![IpAddr::from([93, 184, 216, 34])])
    }
}

/// The example rule with this id, as the app installs it.
#[allow(
    dead_code,
    reason = "each test binary uses a different subset of the harness"
)]
pub fn example(id: &str) -> Rule {
    rd_siterules::examples()
        .into_iter()
        .find(|rule| rule.id == id)
        .unwrap_or_else(|| panic!("the example list carries the rule {id:?}"))
}

pub async fn run(rule: &Rule, fetcher: &Recorded, address: &str) -> Result<Crawl, RunError> {
    let clock = SystemClock::new();
    Executor::new(fetcher, &PublicDns, &clock)
        .run(rule, &Url::parse(address).expect("address"))
        .await
}

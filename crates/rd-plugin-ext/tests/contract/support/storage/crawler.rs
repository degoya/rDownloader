//! The crawler half of the storage harness: the cases a cloud drive's folder crawler answers
//! alike, through hosts that know nothing of any provider's protocol.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    PluginManifest,
    extension::{CrawlRefusal, FolderCrawler},
};

use super::marker;

/// One provider's crawler, as a row of the table.
pub struct Crawler {
    /// The package name of the bundled component.
    pub component: &'static str,
    pub manifest: &'static str,
    /// A folder address the crawler claims, for the cases that need no particular folder.
    pub folder: &'static str,
    /// Addresses it claims, and addresses it leaves alone: the sibling resolver's, every
    /// spelling of them, and somebody else's entirely.
    pub claimed: &'static [&'static str],
    pub foreign: &'static [&'static str],
    /// How the provider refuses a listing, as `(status, body, the code it becomes)`.
    pub refusals: &'static [(u16, &'static str, &'static str)],
    /// What the provider wrote beside its refusals; none of it may reach one.
    pub prose: &'static [&'static str],
}

/// A bundled component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
pub fn component(row: &Crawler) -> Vec<u8> {
    rd_plugin_host::artifact::component(row.component)
}

/// The crawler of `row` in front of `host`.
pub fn build(row: &Crawler, bytes: &[u8], host: Arc<dyn ResolverHost>) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(row.manifest).expect("the crawler manifest");
    FolderCrawler::new(manifest, bytes, Some(host)).expect("compile the crawler")
}

/// Answers every request with one status and one body, `Retry-After: 90` beside it, and counts
/// what it was asked.
pub struct Answering {
    status: u16,
    body: &'static str,
    requests: AtomicUsize,
}

impl Answering {
    pub fn new(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            status,
            body,
            requests: AtomicUsize::new(0),
        })
    }

    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ResolverHost for Answering {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(HostHttpResponse {
            status: self.status,
            final_url: request.url,
            headers: vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
            body: self.body.as_bytes().to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

/// The sandbox's answer to an address outside the declared domains, for every request.
struct Refusing;

#[async_trait]
impl ResolverHost for Refusing {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        _request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.http_target_not_allowed",
            "outside the declared domains",
        ))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// The check that keeps a crawler from costing every other link in the queue a request, and
/// the one that keeps the siblings apart: a file address is the resolver's.
pub async fn claims_only_its_own_addresses(row: &Crawler) {
    let bytes = component(row);
    let host = Answering::new(404, "");
    let crawler = build(row, &bytes, host.clone());
    for claimed in row.claimed {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in row.foreign {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert_eq!(
        host.requests(),
        0,
        "deciding whether a link is claimed must reach nothing"
    );
}

/// Every refusal of the row, each through a host that answers nothing else: its own code, none
/// of the provider's prose, and what each refusal took in requests, for a provider that asserts
/// more.
pub async fn refusals(row: &Crawler) -> Vec<(CrawlRefusal, usize)> {
    let bytes = component(row);
    let mut seen = Vec::new();
    for (status, body, expected) in row.refusals {
        let host = Answering::new(*status, body);
        let refusal = build(row, &bytes, host.clone())
            .crawl(row.folder, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(*expected), "{status}");
        let shown = format!("{refusal:?}");
        for sentence in row.prose {
            assert!(
                !shown.contains(*sentence),
                "a provider's prose reached the refusal: {shown}"
            );
        }
        seen.push((refusal, host.requests()));
    }
    seen
}

/// The ways a crawl can fail reach the person as different codes, and none of them repeats a
/// word the provider wrote.
pub async fn refusals_are_told_apart(row: &Crawler) {
    refusals(row).await;
}

/// A crawler reaches nothing outside the domains its own manifest declares.
pub async fn reaches_nothing_its_manifest_did_not_declare(row: &Crawler) {
    let bytes = component(row);
    let refusal = build(row, &bytes, Arc::new(Refusing))
        .crawl(row.folder, None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

/// Every `Authorization` header a mock saw names the vault reference and never holds the token.
pub fn assert_bearer_markers(sent: &[String], reference: &str) {
    assert!(!sent.is_empty());
    let expected = format!("Bearer {}", marker(reference));
    for value in sent {
        assert_eq!(
            value, &expected,
            "the plugin must name the reference, never hold the token"
        );
    }
}

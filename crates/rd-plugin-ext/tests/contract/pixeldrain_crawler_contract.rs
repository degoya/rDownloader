//! The crawler contract, exercised against the bundled Pixeldrain list crawler
//! (RD-191-07, PLUG-23).
//!
//! `pixeldrain-crawler` had unit tests of its list reader and nothing that ran the component.
//! What only the component can show: that a list address is claimed without a request, that
//! one `GET /api/list/{id}` is all it makes, that the files come back as the `/u/` addresses
//! the sibling resolver takes, and that every refusal — a token in the document, a bare status,
//! an empty list — arrives with a code the interface can translate.
//!
//! **Pixeldrain is a mock.** It answers at the host boundary, so no socket is opened and
//! pixeldrain.com is not contacted.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_host::{PluginManifest, extension::FolderCrawler};

const MANIFEST: &str = include_str!("../../../../plugins/pixeldrain-crawler/manifest.toml");

const LIST: &str = "https://pixeldrain.com/l/Lm4pQ2";

fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-pixeldrain-crawler")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the crawler manifest")
}

/// One canned answer for every request, and a record of what was asked.
struct MockPixeldrain {
    status: u16,
    body: &'static str,
    requests: Mutex<Vec<String>>,
}

impl MockPixeldrain {
    fn answering(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            status,
            body,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ResolverHost for MockPixeldrain {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {}", request.method, request.url));
        Ok(HostHttpResponse {
            status: self.status,
            final_url: request.url.clone(),
            headers: Vec::new(),
            body: self.body.as_bytes().to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

fn crawler(host: Arc<MockPixeldrain>, bytes: &[u8]) -> FolderCrawler {
    FolderCrawler::new(manifest(), bytes, Some(host)).expect("compile the crawler")
}

/// A crawler is asked before it is handed anything, and answers from the address alone. A
/// single file is the resolver's, so exactly one of the two plugins answers for any address.
#[tokio::test]
async fn only_list_addresses_are_claimed_and_without_a_request() {
    let bytes = component();
    let host = MockPixeldrain::answering(200, "{}");
    let crawler = crawler(Arc::clone(&host), &bytes);
    for claimed in [LIST, "https://www.pixeldrain.com/api/list/Lm4pQ2"] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        "https://pixeldrain.com/u/Ab3xY9Zq",
        "https://pixeldrain.com.evil.test/l/Lm4pQ2",
        "https://example.com/l/Lm4pQ2",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(host.requests().is_empty(), "claiming must reach nothing");
}

#[tokio::test]
async fn a_list_yields_one_file_address_per_entry_under_its_title() {
    let bytes = component();
    let host = MockPixeldrain::answering(
        200,
        r#"{"success":true,"id":"Lm4pQ2","title":"Season 1","file_count":2,
            "files":[{"id":"Ab3xY9Zq","name":"a.bin","size":4096},
                     {"id":"Cd7wV1Nk","name":"b.bin","size":8192}]}"#,
    );
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(LIST, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(links[0].url, "https://pixeldrain.com/u/Ab3xY9Zq");
    assert_eq!(links[0].file_name.as_deref(), Some("a.bin"));
    assert_eq!(links[0].size, Some(4096));
    assert_eq!(links[0].package_hint.as_deref(), Some("Season 1"));
    assert_eq!(links[1].url, "https://pixeldrain.com/u/Cd7wV1Nk");
    // One request, against the list endpoint, and no link it found is followed.
    assert_eq!(
        host.requests(),
        vec!["GET https://pixeldrain.com/api/list/Lm4pQ2".to_owned()]
    );
}

/// An empty answer is a refusal with its own code, never a package with nothing in it.
#[tokio::test]
async fn an_empty_list_is_reported_and_not_silently_dropped() {
    let bytes = component();
    let host = MockPixeldrain::answering(200, r#"{"success":true,"title":"x","files":[]}"#);
    let refusal = crawler(host, &bytes)
        .crawl(LIST, None)
        .await
        .expect("call")
        .expect_err("an empty list is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("pixeldrain_crawler.list_empty")
    );
}

/// The document's token decides before the status: it tells a deleted list from a blocked one,
/// and an unknown token travels as a parameter, never as Pixeldrain's sentence.
#[tokio::test]
async fn a_token_in_the_document_decides_the_refusal() {
    let bytes = component();
    for (status, body, code) in [
        (
            404,
            r#"{"success":false,"value":"not_found","message":"gone"}"#,
            "pixeldrain_crawler.list_not_found",
        ),
        (
            403,
            r#"{"success":false,"value":"authentication_required"}"#,
            "pixeldrain_crawler.account_required",
        ),
        (
            429,
            r#"{"success":false,"value":"ip_rate_limit_reached"}"#,
            "pixeldrain_crawler.rate_limited",
        ),
        (
            400,
            r#"{"success":false,"value":"something_new"}"#,
            "pixeldrain_crawler.api_error",
        ),
    ] {
        let refusal = crawler(MockPixeldrain::answering(status, body), &bytes)
            .crawl(LIST, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(code), "{body}");
        assert!(!refusal.message.contains("gone"), "{}", refusal.message);
    }
}

/// Without a document to read, the status decides.
#[tokio::test]
async fn a_bare_status_is_refused_with_the_code_that_fits_it() {
    let bytes = component();
    for (status, code) in [
        (401, "pixeldrain_crawler.account_required"),
        (404, "pixeldrain_crawler.list_not_found"),
        (429, "pixeldrain_crawler.rate_limited"),
        (503, "pixeldrain_crawler.server_error"),
    ] {
        let refusal = crawler(MockPixeldrain::answering(status, "<html>no</html>"), &bytes)
            .crawl(LIST, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(code), "{status}");
    }
}

#[test]
fn the_crawler_reaches_only_pixeldrain() {
    let manifest = manifest();
    let http = manifest
        .capabilities
        .net_http
        .as_ref()
        .expect("the crawler needs HTTP");
    assert_eq!(http.domains, ["pixeldrain.com", "www.pixeldrain.com"]);
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
}

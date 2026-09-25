//! The crawler contract, exercised end to end against the bundled Premiumize crawler.
//!
//! What is proven here is what a person pasting a folder address is promised, not what the
//! plugin would like: that a link belonging to somebody else is never fetched, that a folder
//! comes back with its names, its sizes and its structure, that a tree cannot be walked for
//! ever, and — the defect this job exists to fix — that a folder holding nothing says so with
//! a code the interface can translate instead of quietly producing no links.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! premiumize.me is not contacted. A run against a real Premiumize account with a counted
//! number of files is *not* claimed here; it needs an account this checkout does not have.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_host::{PluginManifest, extension::FolderCrawler};

const MANIFEST: &str = include_str!("../../../plugins/premiumize-crawler/manifest.toml");

/// The bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-premiumize-crawler")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the crawler manifest")
}

/// A folder as the mock holds it: one `folder/list` document, keyed by id.
struct Folder {
    body: String,
}

/// The mock Premiumize API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// key left the plugin as the template `{{secret:premiumize_api_key}}` and never as a value.
struct MockPremiumize {
    folders: Vec<(&'static str, Folder)>,
    requests: Mutex<Vec<String>>,
    /// Every `Authorization` header value the plugin sent, verbatim.
    authorizations: Mutex<Vec<String>>,
    /// When set, every request fails with this status instead of being answered.
    status: Option<u16>,
}

impl MockPremiumize {
    fn new(folders: Vec<(&'static str, Folder)>) -> Arc<Self> {
        Arc::new(Self {
            folders,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            status: None,
        })
    }

    fn failing(status: u16) -> Arc<Self> {
        Arc::new(Self {
            folders: Vec::new(),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            status: Some(status),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

/// One `folder/list` answer.
fn listing(name: &'static str, entries: &[String]) -> Folder {
    Folder {
        body: format!(
            r#"{{"status":"success","name":"{name}","content":[{}]}}"#,
            entries.join(",")
        ),
    }
}

fn file(name: &str, size: u64) -> String {
    format!(
        r#"{{"id":"{name}","name":"{name}","type":"file","size":{size},
            "link":"https://8.premiumize.me/dl/{name}"}}"#
    )
}

fn subfolder(id: &str, name: &str) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","type":"folder"}}"#)
}

#[async_trait]
impl ResolverHost for MockPremiumize {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
        }
        let id = request
            .query
            .iter()
            .find(|value| value.name == "id")
            .map(|value| value.value_template.clone())
            .unwrap_or_default();
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {}", request.url.path(), id));
        let answer = |status: u16, body: String| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers: Vec::new(),
                body: body.into_bytes(),
            })
        };
        if let Some(status) = self.status {
            return answer(status, r#"{"status":"error","message":"nope"}"#.to_owned());
        }
        match self.folders.iter().find(|(known, _)| *known == id) {
            Some((_, folder)) => answer(200, folder.body.clone()),
            None => answer(
                200,
                r#"{"status":"error","message":"not found"}"#.to_owned(),
            ),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn crawler(host: Arc<MockPremiumize>, bytes: &[u8]) -> FolderCrawler {
    FolderCrawler::new(manifest(), bytes, Some(host)).expect("compile the crawler")
}

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// This is the check that keeps a crawler from costing every other link in the queue a
/// request: the resolver beside it claims every http(s) address, because a multihoster is
/// chosen by catalogue rather than by domain.
#[tokio::test]
async fn only_premiumize_folder_addresses_are_claimed() {
    let bytes = component();
    let host = MockPremiumize::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        "https://www.premiumize.me/folder/abc123",
        "https://premiumize.me/folder?id=abc123",
        "https://www.premiumize.me/files?folder_id=abc123",
        "https://www.premiumize.me/item/abc123",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        "https://ddownload.com/f/abc123",
        "https://premiumize.me.evil.test/folder/abc123",
        "https://www.premiumize.me/account/info",
        "https://8.premiumize.me/dl/e01.mkv",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.requests().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A folder comes back as its files, with the names, the sizes and the structure the API gave.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = component();
    let host = MockPremiumize::new(vec![
        (
            "root",
            listing(
                "Show",
                &[file("readme.txt", 12), subfolder("s1", "Season 1")],
            ),
        ),
        (
            "s1",
            listing("Season 1", &[file("e01.mkv", 1024), file("e02.mkv", 2048)]),
        ),
    ]);
    let crawler = crawler(Arc::clone(&host), &bytes);

    let links = crawler
        .crawl("https://www.premiumize.me/folder/root", None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("readme.txt"));
    assert_eq!(links[0].size, Some(12));
    // The crawled folder's own name is the package suggestion; a subfolder extends it.
    assert_eq!(links[0].package_hint.as_deref(), Some("Show"));
    assert_eq!(links[1].file_name.as_deref(), Some("e01.mkv"));
    assert_eq!(links[1].size, Some(1024));
    assert_eq!(links[1].package_hint.as_deref(), Some("Show/Season 1"));
    assert_eq!(links[2].package_hint.as_deref(), Some("Show/Season 1"));
    assert_eq!(
        host.requests(),
        vec![
            "/api/folder/list root".to_owned(),
            "/api/folder/list s1".to_owned()
        ]
    );
}

/// The account's key never leaves the plugin as a value.
#[tokio::test]
async fn the_account_key_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockPremiumize::new(vec![("root", listing("Show", &[file("a.mkv", 1)]))]);
    let crawler = crawler(Arc::clone(&host), &bytes);

    crawler
        .crawl("https://www.premiumize.me/folder/root", None)
        .await
        .expect("call")
        .expect("a listing");
    let authorizations = host.authorizations.lock().expect("authorizations").clone();
    assert_eq!(
        authorizations,
        vec!["Bearer {{secret:premiumize_api_key}}".to_owned()],
        "the plugin must name the reference, never hold the key"
    );
}

/// A folder that contains itself is read once, not for ever.
///
/// The failure this prevents cannot be spotted by watching: every single request the walk
/// makes is perfectly reasonable on its own.
#[tokio::test]
async fn a_cycle_is_walked_once() {
    let bytes = component();
    let host = MockPremiumize::new(vec![
        (
            "root",
            listing("Loop", &[subfolder("inner", "Inner"), file("a.mkv", 1)]),
        ),
        (
            "inner",
            listing(
                "Inner",
                &[
                    subfolder("root", "Back"),
                    subfolder("inner", "Self"),
                    file("b.mkv", 2),
                ],
            ),
        ),
    ]);
    let crawler = crawler(Arc::clone(&host), &bytes);

    let links = crawler
        .crawl("https://www.premiumize.me/folder/root", None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(host.requests().len(), 2, "each folder is read exactly once");
}

/// An empty folder says so, with a code the interface can translate.
///
/// The whole point of the job: before it, a Premiumize folder ended in a message telling the
/// person to split the source in the LinkGrabber, where nothing could.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = component();
    let host = MockPremiumize::new(vec![("root", listing("Empty", &[]))]);
    let crawler = crawler(host, &bytes);

    let refusal = crawler
        .crawl("https://www.premiumize.me/folder/root", None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("premiumize_crawler.folder_empty")
    );
}

/// A folder that cannot be read says that, and says it differently from an empty one.
#[tokio::test]
async fn an_unreachable_folder_is_reported_with_its_own_code() {
    let bytes = component();
    let gone = MockPremiumize::new(Vec::new());
    let refusal = crawler(gone, &bytes)
        .crawl("https://www.premiumize.me/folder/missing", None)
        .await
        .expect("call")
        .expect_err("a missing folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("premiumize_crawler.folder_unreachable")
    );

    let refused = MockPremiumize::failing(401);
    let refusal = crawler(refused, &bytes)
        .crawl("https://www.premiumize.me/folder/root", None)
        .await
        .expect("call")
        .expect_err("a refused account is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("premiumize_crawler.api_key_required")
    );
}

/// A crawler reaches nothing outside the domains its own manifest declares.
#[tokio::test]
async fn the_crawler_reaches_nothing_its_manifest_did_not_declare() {
    let bytes = component();
    // A host that refuses everything stands in for "the sandbox said no": what matters is
    // that the failure is reported rather than turned into an empty folder.
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
    let crawler = FolderCrawler::new(manifest(), &bytes, Some(Arc::new(Refusing)))
        .expect("compile the crawler");
    let refusal = crawler
        .crawl("https://www.premiumize.me/folder/root", None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

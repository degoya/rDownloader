//! pCloud, exercised end to end against a mock pCloud API that has **two installations**, and
//! a mock pCloud authorization server (RD-120-06).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither `pcloud.com` nor `pcloud.link` is contacted. A run against a real pCloud account is
//! *not* claimed here; there is none in this checkout, and there is no registered application
//! either. The fixtures beside this file are written to pCloud's **documented** answer shapes
//! rather than recorded from a live account, and every token-shaped value in them is a
//! placeholder — `fixtures_carry_no_credential_material` is what keeps it that way.
//!
//! Two of the three pCloud plugins are driven here as real WebAssembly components, because
//! that is the only place their guest code exists: the crawler's walk and the sign-in's
//! exchange are written against the WIT imports and cannot run natively. The third,
//! `plugins/pcloud/`, keeps its protocol logic outside the component and is covered by
//! `plugins/pcloud/src/native/tests.rs` against the same kind of mock.
//!
//! The case this file exists for above all others is the **region**. The mock holds the
//! account, the files and the link codes in exactly one of pCloud's two installations and
//! answers the other one the way pCloud does — `result: 2094` for a token it will not accept,
//! `result: 7001` for a link code it never issued, both under HTTP 200. A plugin that read
//! either at face value would report a bad credential or a dead link, which is precisely the
//! mistake that is impossible to tell from the real thing in production.
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | The same folder, addressed at the wrong installation | one correction, then pinned |
//! | A public folder link | one `showpublink`, its whole tree walked under the same limits |
//! | A public link holding one file | that one file, spelled with its `fileid` |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Refused token, rate limit, missing folder | three different codes, no provider prose |
//! | The sign-in | a code through to a stored token, at whichever installation issued it |
//! | The renewal | refused, because pCloud issues nothing to renew with |
//! | The device entrance | refused with a stable code; the manifest offers it to nobody |
//! | The application key and secret | markers in the address and the exchange, never values |

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    OAuthFlowManifest, PluginManifest,
    extension::{FolderCrawler, OAuthProvider, TokenOutcome},
};

const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/pcloud-crawler/manifest.toml");
const OAUTH_MANIFEST: &str = include_str!("../../../plugins/pcloud-oauth/manifest.toml");
const RESOLVER_MANIFEST: &str = include_str!("../../../plugins/pcloud/manifest.toml");

// The sanitised fixtures, written to pCloud's documented answer shapes.
const USER_INFO: &str = include_str!("fixtures/pcloud/userinfo.json");
const LISTFOLDER_ROOT: &str = include_str!("fixtures/pcloud/listfolder_root.json");
const LISTFOLDER_SEASON: &str = include_str!("fixtures/pcloud/listfolder_season.json");
const LISTFOLDER_EMPTY: &str = include_str!("fixtures/pcloud/listfolder_empty.json");
const SHOWPUBLINK_FOLDER: &str = include_str!("fixtures/pcloud/showpublink_folder.json");
const SHOWPUBLINK_FILE: &str = include_str!("fixtures/pcloud/showpublink_file.json");
const TOKEN_SUCCESS: &str = include_str!("fixtures/pcloud/token_success.json");
const TOKEN_INVALID_CODE: &str = include_str!("fixtures/pcloud/token_invalid_code.json");
const TOKEN_RATE_LIMITED: &str = include_str!("fixtures/pcloud/token_rate_limited.json");
const ERROR_NOT_FOUND: &str = include_str!("fixtures/pcloud/error_file_not_found.json");
const ERROR_INVALID_TOKEN: &str = include_str!("fixtures/pcloud/error_invalid_access_token.json");
const ERROR_TOO_MANY: &str = include_str!("fixtures/pcloud/error_too_many_requests.json");
const ERROR_LINK: &str = include_str!("fixtures/pcloud/error_link_unavailable.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
/// The sentence every fixture's `error` carries. Nothing that reaches a person may contain it.
const PROVIDER_PROSE: &str = "A sentence pCloud wrote";

const US_API: &str = "api.pcloud.com";
const EU_API: &str = "eapi.pcloud.com";

/// The public link every shared case uses, spelled on the European short host.
const PUBLIC_LINK_EU: &str = "https://e.pcloud.link/publink/show?code=XZredacted";
/// The same code on the host pCloud's own `getfilepublink` hands back, which names no region.
const PUBLIC_LINK_UNSPECIFIED: &str = "https://my.pcloud.com/#page=publink&code=XZredacted";

/// A bundled component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn crawler_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-pcloud-crawler")
}

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-pcloud-oauth")
}

// -- the mock pCloud, in two installations ---------------------------------------------------

/// One recorded request: the installation it went to, the method, and its parameters.
#[derive(Clone, Debug)]
struct Sent {
    host: String,
    method: String,
    parameters: Vec<(String, String)>,
}

impl Sent {
    fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The mock pCloud API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:pcloud_access_token}}` and never as a value.
///
/// Exactly one of pCloud's two installations holds anything. The other answers the way pCloud
/// does: 2094 to a token, 7001 to a link code, both under HTTP 200.
struct MockPCloud {
    home: &'static str,
    /// When set, every method answers this document instead of its own answer.
    refusal: Option<&'static str>,
    /// What `showpublink` answers with — a folder's tree, or one file.
    publink: &'static str,
    /// Whether the crawled folder holds anything.
    empty: bool,
    requests: Mutex<Vec<Sent>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockPCloud {
    fn new(home: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            refusal: None,
            publink: SHOWPUBLINK_FOLDER,
            empty: false,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn with_publink(home: &'static str, publink: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            refusal: None,
            publink,
            empty: false,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn empty_folder() -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            refusal: None,
            publink: SHOWPUBLINK_FOLDER,
            empty: true,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn refusing(body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            refusal: Some(body),
            publink: SHOWPUBLINK_FOLDER,
            empty: false,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Sent> {
        self.requests.lock().expect("requests").clone()
    }

    fn route(&self) -> Vec<(String, String)> {
        self.requests()
            .into_iter()
            .map(|sent| (sent.host, sent.method))
            .collect()
    }
}

#[async_trait]
impl ResolverHost for MockPCloud {
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
        let host = request.url.host_str().unwrap_or_default().to_owned();
        let method = request.url.path().trim_start_matches('/').to_owned();
        let parameters: Vec<(String, String)> = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        self.requests.lock().expect("requests").push(Sent {
            host: host.clone(),
            method: method.clone(),
            parameters: parameters.clone(),
        });
        let answer = |body: &str| {
            Ok(HostHttpResponse {
                // pCloud answers HTTP 200 to its own refusals too, which is the whole point.
                status: 200,
                final_url: request.url.clone(),
                headers: vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
                body: body.as_bytes().to_vec(),
            })
        };
        if let Some(body) = self.refusal {
            return answer(body);
        }
        // The other installation knows nothing of this account.
        if host != self.home {
            return answer(if method.contains("publink") {
                ERROR_LINK
            } else {
                ERROR_INVALID_TOKEN
            });
        }
        match method.as_str() {
            "userinfo" => answer(USER_INFO),
            "showpublink" => answer(self.publink),
            "listfolder" => match parameters
                .iter()
                .find(|(name, _)| name == "folderid")
                .map(|(_, value)| value.as_str())
            {
                _ if self.empty => answer(LISTFOLDER_EMPTY),
                Some("42") => answer(LISTFOLDER_ROOT),
                Some("43") => answer(LISTFOLDER_SEASON),
                _ => answer(ERROR_NOT_FOUND),
            },
            _ => answer(ERROR_NOT_FOUND),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn crawler(host: Arc<MockPCloud>, bytes: &[u8]) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the crawler manifest");
    FolderCrawler::new(manifest, bytes, Some(host)).expect("compile the crawler")
}

// -- the crawler -----------------------------------------------------------------------------

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// The check that keeps a crawler from costing every other link in the queue a request — and,
/// here, the one that keeps the two siblings apart. pCloud gives no handle but `fileid`: a
/// link code is opaque, so an address carrying one is the resolver's and an address without
/// one is this plugin's, whatever the link turns out to hold.
#[tokio::test]
async fn only_pcloud_folder_and_link_addresses_are_claimed() {
    let bytes = crawler_component();
    let host = MockPCloud::new(US_API);
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        "https://my.pcloud.com/#/filemanager?folder=42",
        "https://e.pcloud.com/#/filemanager?folder=0",
        PUBLIC_LINK_EU,
        PUBLIC_LINK_UNSPECIFIED,
        "https://u.pcloud.link/publink/show?code=XZredacted",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // The sibling resolver's addresses, every spelling of them.
        "https://my.pcloud.com/#/filemanager?folder=42&fileid=5001",
        "https://e.pcloud.link/publink/show?code=XZredacted&fileid=7",
        // And somebody else's entirely.
        "https://pcloud.com.evil.test/#/filemanager?folder=42",
        "https://api.pcloud.com/listfolder?folderid=42",
        "https://ddownload.com/f/abc",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.requests().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A folder comes back as its files, with the names, the sizes and the structure pCloud gave —
/// and each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockPCloud::new(US_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("readme.txt"));
    assert_eq!(links[0].size, Some(12));
    // The crawled folder's own name is the package suggestion; a subfolder extends it.
    assert_eq!(links[0].package_hint.as_deref(), Some("Show"));
    assert_eq!(links[1].file_name.as_deref(), Some("e01.mkv"));
    assert_eq!(links[1].package_hint.as_deref(), Some("Show/Season 1"));
    // The one thing that crosses between the two packages: an address, and nothing else — and
    // it carries the installation that answered, so the resolver never pays the correction.
    assert_eq!(
        links[1].url,
        "https://my.pcloud.com/#/filemanager?folder=43&fileid=5002"
    );
    // The row without an identifier was dropped, not guessed at.
    assert!(
        links
            .iter()
            .all(|link| link.file_name.as_deref() != Some("no identifier"))
    );
    assert_eq!(
        host.route(),
        vec![
            (US_API.to_owned(), "listfolder".to_owned()),
            (US_API.to_owned(), "listfolder".to_owned()),
        ],
        "one call per folder and no more: pCloud paginates none of this"
    );
    assert_eq!(host.requests()[0].parameter("folderid"), Some("42"));
    assert_eq!(host.requests()[1].parameter("folderid"), Some("43"));
}

/// **The region.** The address names one installation, the account lives in the other, and
/// pCloud answers the first exactly as it answers a bad token. One correction, and then the
/// installation that answered is pinned for the whole walk.
#[tokio::test]
async fn a_folder_in_the_other_installation_is_found_after_one_correction() {
    let bytes = crawler_component();
    let host = MockPCloud::new(EU_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    // Every address handed back names the installation that actually answered, so nothing
    // downstream repeats the correction.
    assert!(
        links
            .iter()
            .all(|link| link.url.starts_with("https://e.pcloud.com/")),
        "{links:?}"
    );
    assert_eq!(
        host.route(),
        vec![
            // The address said the United States, so that is where it started.
            (US_API.to_owned(), "listfolder".to_owned()),
            // 2094 there, corrected once.
            (EU_API.to_owned(), "listfolder".to_owned()),
            // And pinned: the subfolder pays nothing.
            (EU_API.to_owned(), "listfolder".to_owned()),
        ],
        "the correction must happen once and then stop happening"
    );
}

/// A public folder link is read from the **one** tree pCloud answers with, under the same
/// limits an own-drive walk runs under — a tree a stranger shared is no more trustworthy for
/// having arrived in one piece.
#[tokio::test]
async fn a_public_folder_link_is_read_from_the_one_tree_pcloud_answers_with() {
    let bytes = crawler_component();
    let host = MockPCloud::new(EU_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(PUBLIC_LINK_EU, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("a.bin"));
    assert_eq!(links[0].package_hint.as_deref(), Some("Shared"));
    assert_eq!(links[1].package_hint.as_deref(), Some("Shared/Sub"));
    assert_eq!(
        links[1].url,
        "https://e.pcloud.link/publink/show?code=XZredacted&fileid=9003"
    );
    assert_eq!(
        host.route(),
        vec![(EU_API.to_owned(), "showpublink".to_owned())],
        "pCloud answers a public folder link with its whole tree, so one call is all of it"
    );
    // A public link needs no credential, and none is sent.
    assert!(
        host.authorizations
            .lock()
            .expect("authorizations")
            .is_empty(),
        "a public link needs no credential, so none may be sent"
    );
}

/// The address pCloud's own `getfilepublink` hands back names no installation, so the link
/// code is looked for in one and then the other — and `7001` at the first is what says so.
#[tokio::test]
async fn a_public_link_that_names_no_installation_is_found_in_the_other_one() {
    let bytes = crawler_component();
    let host = MockPCloud::new(EU_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(PUBLIC_LINK_UNSPECIFIED, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(
        host.route(),
        vec![
            (US_API.to_owned(), "showpublink".to_owned()),
            (EU_API.to_owned(), "showpublink".to_owned()),
        ]
    );
    // And the addresses it hands back name the installation that answered.
    assert!(
        links
            .iter()
            .all(|link| link.url.starts_with("https://e.pcloud.link/")),
        "{links:?}"
    );
}

/// A public link holding a single file is still this plugin's — no address could have said
/// otherwise — and comes back as that one file, spelled with its `fileid`.
#[tokio::test]
async fn a_public_link_to_one_file_comes_back_as_that_one_file() {
    let bytes = crawler_component();
    let host = MockPCloud::with_publink(EU_API, SHOWPUBLINK_FILE);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(PUBLIC_LINK_EU, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("single.bin"));
    assert_eq!(links[0].size, Some(33));
    assert_eq!(
        links[0].url,
        "https://e.pcloud.link/publink/show?code=XZredacted&fileid=9100"
    );
    // One file is not a package.
    assert_eq!(links[0].package_hint, None);
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockPCloud::new(US_API);
    crawler(Arc::clone(&host), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect("a listing");

    let authorizations = host.authorizations.lock().expect("authorizations").clone();
    assert!(!authorizations.is_empty());
    for value in authorizations {
        assert_eq!(
            value, "Bearer {{secret:pcloud_access_token}}",
            "the plugin must name the reference, never hold the token"
        );
    }
}

/// An empty folder says so, with a code the interface can translate.
///
/// Returning an empty list would create a package with nothing in it and nothing to explain
/// why — the defect ADR 0001 was written for.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let refusal = crawler(MockPCloud::empty_folder(), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=44", None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(refusal.code.as_deref(), Some("pcloud_crawler.folder_empty"));
}

/// The ways a crawl can fail reach the person as different codes, none of them repeating a
/// word pCloud wrote — the number travels instead, because a number cannot carry a token.
#[tokio::test]
async fn a_refused_token_a_rate_limit_and_a_missing_folder_are_told_apart() {
    let bytes = crawler_component();
    for (body, expected) in [
        (ERROR_INVALID_TOKEN, "pcloud_crawler.sign_in_required"),
        (ERROR_TOO_MANY, "pcloud_crawler.rate_limited"),
        (ERROR_NOT_FOUND, "pcloud_crawler.folder_unreachable"),
        (ERROR_LINK, "pcloud_crawler.link_unavailable"),
    ] {
        let host = MockPCloud::refusing(body);
        let refusal = crawler(Arc::clone(&host), &bytes)
            .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(expected), "{body}");
        assert!(
            !format!("{refusal:?}").contains(PROVIDER_PROSE),
            "a provider's prose reached the failure: {refusal:?}"
        );
        // A rate limit and a missing folder are not region questions and are asked once; a
        // refused token and a refused link code are, and are asked exactly twice.
        let expected_requests = if body == ERROR_INVALID_TOKEN || body == ERROR_LINK {
            2
        } else {
            1
        };
        assert_eq!(host.requests().len(), expected_requests, "{expected}");
    }
}

/// A crawler reaches nothing outside the domains its own manifest declares.
#[tokio::test]
async fn the_crawler_reaches_nothing_its_manifest_did_not_declare() {
    let bytes = crawler_component();
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
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the crawler manifest");
    let refusal = FolderCrawler::new(manifest, &bytes, Some(Arc::new(Refusing)))
        .expect("compile the crawler")
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

// -- the sign-in -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    url: String,
    form: Vec<(String, String)>,
}

impl Recorded {
    fn field(&self, name: &str) -> Option<&str> {
        self.form
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Stored {
    account_id: AccountId,
    access_token: String,
    refresh_token: Option<String>,
    expires_in_seconds: Option<u64>,
}

/// The mock pCloud authorization server, in two installations.
struct MockAuthorizer {
    /// The installation that issued the code, and the only one that will redeem it.
    home: &'static str,
    /// When set, both installations answer this instead.
    refusal: Option<&'static str>,
    /// Whether the account carries a registered application.
    registered: bool,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockAuthorizer {
    fn new(home: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            refusal: None,
            registered: true,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn refusing(body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            refusal: Some(body),
            registered: true,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn unregistered() -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            refusal: None,
            registered: false,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn stored(&self) -> Vec<Stored> {
        self.stored.lock().expect("stored").clone()
    }
}

#[async_trait]
impl ResolverHost for MockAuthorizer {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let form: Vec<(String, String)> = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            url: request.url.to_string(),
            form,
        });
        let host = request.url.host_str().unwrap_or_default().to_owned();
        let body = self.refusal.unwrap_or(if host == self.home {
            TOKEN_SUCCESS
        } else {
            // The installation that did not issue this code does not know it.
            TOKEN_INVALID_CODE
        });
        Ok(HostHttpResponse {
            // pCloud answers HTTP 200 to a refused exchange too.
            status: 200,
            final_url: request.url.clone(),
            headers: vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
            body: body.as_bytes().to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.registered || reference != "pcloud_client_secret"
    }

    async fn store_oauth_token(
        &self,
        account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        self.stored.lock().expect("stored").push(Stored {
            account_id,
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.map(str::to_owned),
            expires_in_seconds,
        });
        Ok(())
    }
}

fn provider(host: Arc<MockAuthorizer>, bytes: &[u8]) -> OAuthProvider {
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

fn parameter(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The authorization code, from the address the person is sent to all the way to the stored
/// token — and stored without renewal material, because pCloud issues none.
#[tokio::test]
async fn the_sign_in_runs_through_to_a_stored_token() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(US_API);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    assert!(
        request
            .authorization_url
            .starts_with("https://my.pcloud.com/oauth2/authorize?"),
        "{}",
        request.authorization_url
    );
    assert_eq!(
        parameter(&request.authorization_url, "response_type").as_deref(),
        Some("code")
    );
    assert_eq!(
        parameter(&request.authorization_url, "state").as_deref(),
        Some(request.state.as_str())
    );
    // No application key anywhere in this plugin: the marker travels and the host substitutes
    // what *this installation* registered (RD-106-04, rule 8).
    assert!(
        request
            .authorization_url
            .contains("client_id=%7B%7Bclient_id%7D%7D")
            || request
                .authorization_url
                .contains("client_id={{client_id}}"),
        "the authorization URL must carry the marker, not an application key: {}",
        request.authorization_url
    );
    // pCloud offers no PKCE, so there is deliberately nothing to carry between the two calls,
    // and no challenge is invented to look like there is.
    assert!(!request.authorization_url.contains("code_challenge"));
    assert_eq!(request.flow_state, None);
    assert!(request.state.len() >= 43);

    let outcome = plugin
        .poll(account, "the-code-the-callback-carried", None)
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, "https://api.pcloud.com/oauth2_token");
    assert_eq!(
        exchange.field("code"),
        Some("the-code-the-callback-carried")
    );
    // The markers, and never values. The mock stands where the host's expansion would be, so
    // what it sees is what the plugin wrote.
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    assert_eq!(
        exchange.field("client_secret"),
        Some("{{secret:pcloud_client_secret}}")
    );

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            // pCloud issues neither, and saying so is what keeps the host's renewal sweep away
            // from an account it could never renew.
            refresh_token: None,
            expires_in_seconds: None,
        }]
    );
}

/// **The region, in the sign-in.** pCloud states the account's data centre in the redirect,
/// but the contract hands `poll` the code alone. So the code is offered to one installation
/// and then the other, and a code the first never issued cannot be spent there.
#[tokio::test]
async fn a_code_issued_in_the_other_installation_is_redeemed_there() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(EU_API);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .poll(account, "a-european-code", None)
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);
    assert_eq!(
        server
            .requests()
            .iter()
            .map(|request| request.url.clone())
            .collect::<Vec<_>>(),
        vec![
            "https://api.pcloud.com/oauth2_token".to_owned(),
            "https://eapi.pcloud.com/oauth2_token".to_owned(),
        ]
    );
    assert_eq!(server.stored().len(), 1);
}

/// A code neither installation will take ends the flow, and it took both of them to say so.
#[tokio::test]
async fn a_code_neither_installation_takes_ends_the_flow() {
    let bytes = oauth_component();
    let server = MockAuthorizer::refusing(TOKEN_INVALID_CODE);
    let outcome = provider(server.clone(), &bytes)
        .poll(AccountId::new(), "a-stale-code", None)
        .await
        .expect("poll");
    let TokenOutcome::Failed { message, .. } = outcome else {
        panic!("{outcome:?}");
    };
    // pCloud's own number survives; the sentence beside it does not.
    assert!(message.contains("2094"), "{message}");
    assert!(!message.contains(PROVIDER_PROSE), "{message}");
    assert_eq!(server.requests().len(), 2);
    assert!(server.stored().is_empty(), "nothing may be stored");
}

/// A rate limit is a wait, not a failure, and carries pCloud's own `Retry-After`.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = oauth_component();
    let server = MockAuthorizer::refusing(TOKEN_RATE_LIMITED);
    let outcome = provider(server.clone(), &bytes)
        .poll(AccountId::new(), "a-code", None)
        .await
        .expect("poll");
    assert_eq!(
        outcome,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );
    assert!(server.stored().is_empty(), "a rate limit stores nothing");
    assert_eq!(
        server.requests().len(),
        1,
        "a rate limit is not a region question"
    );
}

/// An account with no registered application is told so before anything is asked of pCloud.
///
/// Without this the person would be sent looking for a fault in a registration they never made.
#[tokio::test]
async fn an_account_without_a_registered_application_is_refused_before_anything_is_asked() {
    let bytes = oauth_component();
    let server = MockAuthorizer::unregistered();
    let failure = provider(server.clone(), &bytes)
        .poll(AccountId::new(), "a-code", None)
        .await
        .expect_err("no application");
    assert!(
        format!("{failure:?}").contains("client_not_configured"),
        "{failure:?}"
    );
    assert!(server.requests().is_empty());
}

/// There is nothing to renew from, and the plugin says so rather than pretending.
#[tokio::test]
async fn a_renewal_is_refused_because_pcloud_issues_nothing_to_renew_with() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(US_API);
    let outcome = provider(server.clone(), &bytes)
        .refresh(AccountId::new(), Some("pcloud_access_token"))
        .await
        .expect("refresh");
    let TokenOutcome::Failed { message, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(message.contains("no renewal material"), "{message}");
    assert!(
        server.requests().is_empty(),
        "a renewal that cannot exist must ask pCloud nothing"
    );
}

/// The device entrance is not offered, and being asked anyway is a stable refusal rather than
/// a trap. pCloud has no device flow at all.
#[tokio::test]
async fn the_device_entrance_is_refused_with_a_stable_code() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(US_API);
    let failure = provider(server.clone(), &bytes)
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("no device entrance");
    assert!(
        format!("{failure:?}").contains("flow_unsupported"),
        "{failure:?}"
    );
    assert!(
        server.requests().is_empty(),
        "a flow that is not offered must ask pCloud nothing"
    );
}

// -- the shape of the three packages ---------------------------------------------------------

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pcloud");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        for field in ["access_token", "auth", "code", "client_secret"] {
            let Some(found) = value.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            assert!(
                found == PLACEHOLDER_ACCESS_TOKEN,
                "{path:?} carries a `{field}` that is not a placeholder"
            );
        }
        // A download ticket is a pre-authorised, per-request path. The fixtures carry the
        // redacted shape only.
        if let Some(path_value) = value.get("path").and_then(serde_json::Value::as_str) {
            assert!(
                path_value.starts_with("/redacted-"),
                "{path:?} carries a real download ticket"
            );
        }
        checked += 1;
    }
    assert!(checked >= 15, "only {checked} fixtures were checked");
}

/// The three siblings reach only pCloud, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying the
/// provider row, one crawler and one sign-in claiming that row's slug, and no capability beyond
/// HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_pcloud_each_one_needs() {
    let resolver: PluginManifest = toml::from_str(RESOLVER_MANIFEST).expect("resolver manifest");
    let crawler: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("crawler manifest");
    let sign_in: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("oauth manifest");

    // Only a resolver manifest may carry a provider row, so the account, its vault references
    // and the slug the other two claim all live in exactly one of the three.
    let provider = resolver.provider.as_ref().expect("the provider row");
    assert_eq!(provider.slug, "pcloud");
    assert_eq!(
        provider.credentials,
        rd_plugin_host::CredentialKindManifest::OAuth
    );
    // The account's username field holds this installation's own application key, and it is
    // required: pCloud's token endpoint is a confidential client and there is no PKCE to fall
    // back on, so the secret beside it is a slot of its own.
    assert!(provider.username_required);
    let slots = provider.secret_slots();
    assert_eq!(
        slots
            .iter()
            .map(|slot| slot.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["pcloud_client_secret", "pcloud_access_token"]
    );
    for slot in &slots {
        assert_eq!(
            slot.domains,
            vec![US_API.to_owned(), EU_API.to_owned()],
            "{}",
            slot.reference
        );
    }
    // The bytes come from content servers pCloud picks per request, so they are named by their
    // common suffix and are deliberately **not** among the hosts the token may be sent to: the
    // ticket is already authorised, and `provider_download_bearer` must never fire for pCloud.
    assert_eq!(
        resolver.download_domains,
        vec!["*.pcloud.com".to_owned()],
        "pCloud's content servers cannot be enumerated"
    );
    for slot in &slots {
        assert!(
            !slot.domains.iter().any(|host| host.contains('*')),
            "a credential may only ever go to an exact host"
        );
    }
    assert!(crawler.provider.is_none() && sign_in.provider.is_none());

    for manifest in [&crawler, &sign_in] {
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec!["pcloud".to_owned()], "{}", manifest.name);
    }

    // Both installations for the two that talk to the API; pCloud's own sign-in host as well
    // for the third.
    assert_eq!(
        resolver.capabilities.domains(),
        [US_API.to_owned(), EU_API.to_owned()].as_slice()
    );
    assert_eq!(
        crawler.capabilities.domains(),
        [US_API.to_owned(), EU_API.to_owned()].as_slice()
    );
    assert_eq!(
        sign_in.capabilities.domains(),
        [
            "my.pcloud.com".to_owned(),
            US_API.to_owned(),
            EU_API.to_owned()
        ]
        .as_slice()
    );
    // Both installations are matched, because a pCloud address names one in its host.
    for host in [
        "my.pcloud.com",
        "e.pcloud.com",
        "u.pcloud.link",
        "e.pcloud.link",
    ] {
        assert!(
            resolver.match_domains.iter().any(|known| known == host),
            "{host}"
        );
    }

    // No cookies, no captcha and no raw sockets anywhere: two API clients and a sign-in.
    for manifest in [&resolver, &crawler, &sign_in] {
        assert!(!manifest.capabilities.cookies, "{}", manifest.name);
        assert!(!manifest.capabilities.captcha, "{}", manifest.name);
        assert!(
            manifest.capabilities.net_stream.is_none(),
            "{}",
            manifest.name
        );
    }
    // The sign-in may name the application secret and nothing else: it writes the access token
    // through `store-oauth-token` and has no business reading it back.
    assert_eq!(
        sign_in.capabilities.secrets,
        vec!["pcloud_client_secret".to_owned()]
    );
    // The crawler may spend the token but never the application secret.
    assert_eq!(
        crawler.capabilities.secrets,
        vec!["pcloud_access_token".to_owned()]
    );

    // Redirect only, and stated rather than guessed at, so the host never calls the other.
    assert_eq!(
        sign_in.oauth_flows(),
        [OAuthFlowManifest::Redirect].as_slice()
    );
    assert!(!sign_in.serves_oauth_flow(OAuthFlowManifest::Device));
}

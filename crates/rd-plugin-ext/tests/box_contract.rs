//! Box, exercised end to end against a mock Box Content API and a mock Box token endpoint
//! (RD-120-05).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither api.box.com nor account.box.com is contacted. A run against a real Box account is
//! *not* claimed here; there is none in this checkout, and no registered Box application
//! either. What is proven is everything that does not need one.
//!
//! All three Box plugins are driven here as real WebAssembly components, which is what makes
//! the fourth acceptance criterion — shared and account content use the same contract —
//! something a test can state rather than something a reader has to believe: the crawler's
//! answers are handed to the resolver's own `match-url`, and the resolver's answer for a file
//! found in a folder and for the same file found behind a shared link is built by one code
//! path. `plugins/box/src/native/tests.rs` covers the resolver's protocol logic in more detail
//! against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Box answers a page at a time | every offset followed, to the cap |
//! | A shared link that turns out to be one file | that one file |
//! | A password-protected shared link | the password in the `boxapi` header and nowhere else |
//! | A folder that contains itself | read once |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Denied, not found, throttled, refused token | four different codes |
//! | Shared and account content | one resolver, one download route, one version pin |
//! | The browser sign-in | a code exchanged for a stored token and its renewal material |
//! | The application's id and secret | markers in the exchange, never values |

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};
use rd_plugin_host::{
    ComponentResolver, OAuthFlowManifest, PluginManifest, SecretFilledByManifest,
    extension::{AuthorizationRequest, FolderCrawler, OAuthProvider, TokenOutcome},
};

const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/box-crawler/manifest.toml");
const OAUTH_MANIFEST: &str = include_str!("../../../plugins/box-oauth/manifest.toml");
const RESOLVER_MANIFEST: &str = include_str!("../../../plugins/box/manifest.toml");

// The sanitised fixtures. Every token-shaped value in them is a placeholder and nothing else;
// `fixtures_carry_no_credential_material` is what keeps it that way.
const TOKEN_SUCCESS: &str = include_str!("fixtures/box/token_success.json");
const TOKEN_RENEWED: &str = include_str!("fixtures/box/token_renewed.json");
const TOKEN_DENIED: &str = include_str!("fixtures/box/token_denied.json");
const TOKEN_INVALID_GRANT: &str = include_str!("fixtures/box/token_invalid_grant.json");
const TOKEN_INVALID_CLIENT: &str = include_str!("fixtures/box/token_invalid_client.json");
const TOKEN_RATE_LIMITED: &str = include_str!("fixtures/box/token_rate_limited.json");
const ERROR_NOT_FOUND: &str = include_str!("fixtures/box/error_not_found.json");
const ERROR_FORBIDDEN: &str = include_str!("fixtures/box/error_forbidden.json");
const ERROR_UNAUTHORIZED: &str = include_str!("fixtures/box/error_unauthorized.json");
const ERROR_RATE_LIMIT: &str = include_str!("fixtures/box/error_rate_limit.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";

/// A folder in the account's own Box, as its web address is spelled.
const FOLDER: &str = "https://app.box.com/folder/987654321";
/// A shared link, which does not say whether it points at a folder or a file.
const SHARED_LINK: &str = "https://app.box.com/s/abc123def456";
/// The same link with its password, which is the one thing that must not travel anywhere but
/// the `boxapi` header.
const SHARED_LINK_WITH_PASSWORD: &str =
    "https://app.box.com/s/abc123def456?shared_link_password=hunter2";
const PASSWORD: &str = "hunter2";

/// A bundled component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn crawler_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-box-crawler")
}

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-box-oauth")
}

fn resolver_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-box")
}

// -- the mock Box Content API ----------------------------------------------------------------

/// One folder's pages: each entry is one page's rows, as Box would answer them in order.
type Pages = Vec<Vec<String>>;

/// What the crawled address turns out to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Folder,
    File,
}

/// The mock Box API, shared by the crawler and the resolver.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:box_access_token}}` and never as a value, and that
/// a shared link's password left it only inside a `boxapi` header.
struct MockBox {
    root: Root,
    /// `(folder id, its display name, its pages)`.
    folders: Vec<(&'static str, &'static str, Pages)>,
    /// How many rows each folder claims to hold; `None` means "as many as it answered".
    claimed_total: Option<u64>,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    box_apis: Mutex<Vec<Option<String>>>,
    /// When set, every request fails with this status and this body instead of being answered.
    failure: Option<(u16, &'static str)>,
}

impl MockBox {
    fn new(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            root: Root::Folder,
            folders,
            claimed_total: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            box_apis: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    /// A folder that claims to hold far more rows than it will ever answer, which is how a
    /// collection paginates for as long as anybody asks.
    fn endless(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            claimed_total: Some(10_000),
            ..Arc::try_unwrap(Self::new(folders)).ok().expect("one owner")
        })
    }

    fn single_file() -> Arc<Self> {
        Arc::new(Self {
            root: Root::File,
            ..Arc::try_unwrap(Self::new(Vec::new()))
                .ok()
                .expect("one owner")
        })
    }

    fn failing(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            failure: Some((status, body)),
            ..Arc::try_unwrap(Self::new(Vec::new()))
                .ok()
                .expect("one owner")
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    fn box_apis(&self) -> Vec<Option<String>> {
        self.box_apis.lock().expect("box apis").clone()
    }
}

fn file_row(id: &str, name: &str, size: u64) -> String {
    format!(r#"{{"type":"file","id":"{id}","name":"{name}","size":{size},"item_status":"active"}}"#)
}

fn folder_row(id: &str, name: &str) -> String {
    format!(r#"{{"type":"folder","id":"{id}","name":"{name}","item_status":"active"}}"#)
}

/// The file document `/2.0/files/<id>` answers with, pinned to one version.
fn file_document(id: &str, version: &str) -> String {
    format!(
        r#"{{"type":"file","id":"{id}","name":"release.bin","size":1048576,
            "sha1":"aabbccddeeff00112233445566778899aabbccdd",
            "file_version":{{"type":"file_version","id":"{version}"}},
            "item_status":"active"}}"#
    )
}

#[async_trait]
impl ResolverHost for MockBox {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let mut box_api = None;
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
            if header.name.eq_ignore_ascii_case("boxapi") {
                box_api = Some(header.value_template.clone());
            }
        }
        self.box_apis.lock().expect("box apis").push(box_api);
        let answer = |status: u16, body: String| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers: vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
                body: body.into_bytes(),
            })
        };
        if let Some((status, body)) = self.failure {
            self.requests
                .lock()
                .expect("requests")
                .push("failed".to_owned());
            return answer(status, body.to_owned());
        }
        let offset: u64 = request
            .query
            .iter()
            .find(|value| value.name == "offset")
            .and_then(|value| value.value_template.parse().ok())
            .unwrap_or_default();
        let segments: Vec<&str> = request
            .url
            .path_segments()
            .map(Iterator::collect)
            .unwrap_or_default();
        match segments.as_slice() {
            // What a shared link points at. The one endpoint whose job is to say so.
            ["2.0", "shared_items"] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push("shared_items".to_owned());
                answer(
                    200,
                    match self.root {
                        Root::File => file_document("42", "98765"),
                        Root::Folder => {
                            let (id, name, _) = self.folders.first().expect("a root folder");
                            folder_row(id, name)
                        }
                    },
                )
            }
            ["2.0", "folders", id] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("get {id}"));
                match self.folders.iter().find(|(known, _, _)| known == id) {
                    Some((id, name, _)) => answer(200, folder_row(id, name)),
                    None => answer(404, ERROR_NOT_FOUND.to_owned()),
                }
            }
            ["2.0", "folders", id, "items"] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("list {id} {offset}"));
                let Some((_, _, pages)) = self.folders.iter().find(|(known, _, _)| known == id)
                else {
                    return answer(404, ERROR_NOT_FOUND.to_owned());
                };
                let total = self
                    .claimed_total
                    .unwrap_or_else(|| pages.iter().map(|page| page.len() as u64).sum());
                // Box paginates by offset, so the page answered is the one that starts there.
                let mut start = 0;
                for page in pages {
                    if start == offset {
                        return answer(
                            200,
                            format!(
                                r#"{{"total_count":{total},"offset":{offset},"limit":1000,"entries":[{}]}}"#,
                                page.join(",")
                            ),
                        );
                    }
                    start += page.len() as u64;
                }
                answer(
                    200,
                    format!(
                        r#"{{"total_count":{total},"offset":{offset},"limit":1000,"entries":[]}}"#
                    ),
                )
            }
            ["2.0", "files", id] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("file {id}"));
                answer(200, file_document(id, "98765"))
            }
            _ => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("unexpected {}", request.url.path()));
                answer(404, ERROR_NOT_FOUND.to_owned())
            }
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn crawler(host: Arc<MockBox>, bytes: &[u8]) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the crawler manifest");
    FolderCrawler::new(manifest, bytes, Some(host)).expect("compile the crawler")
}

fn resolver(host: Arc<MockBox>, bytes: &[u8]) -> ComponentResolver {
    let manifest: PluginManifest =
        toml::from_str(RESOLVER_MANIFEST).expect("the resolver manifest");
    ComponentResolver::new(manifest, bytes, host).expect("compile the resolver")
}

// -- the crawler -----------------------------------------------------------------------------

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// The check that keeps a crawler from costing every other link in the queue a request — and,
/// here, the one that keeps the two siblings apart: a file link is the resolver's, and the
/// canonical address the crawler itself hands back is never listed again.
#[tokio::test]
async fn only_box_folder_addresses_and_shared_links_are_claimed() {
    let bytes = crawler_component();
    let host = MockBox::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        FOLDER,
        SHARED_LINK,
        SHARED_LINK_WITH_PASSWORD,
        "https://contoso.app.box.com/folder/1",
        "https://app.box.com/s/abc123def456/folder/7",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // The sibling resolver's addresses, both spellings of them.
        "https://app.box.com/file/123456789",
        "https://app.box.com/s/abc123def456/file/42",
        // And somebody else's entirely.
        "https://app.box.com.evil.test/folder/1",
        "https://ddownload.com/f/abc",
        "https://api.box.com/2.0/files/1/content",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.requests().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A folder comes back as its files, with the names, the sizes and the structure Box gave — and
/// each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![
        (
            "987654321",
            "Show",
            vec![vec![
                file_row("11", "readme.txt", 12),
                folder_row("22", "Season 1"),
            ]],
        ),
        (
            "22",
            "Season 1",
            vec![vec![
                file_row("33", "e01.mkv", 1024),
                file_row("34", "e02.mkv", 2048),
            ]],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
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
    // The one thing that crosses between the two packages: an address, and nothing else.
    assert_eq!(links[1].url, "https://app.box.com/file/33");
    assert_eq!(
        host.requests(),
        vec![
            "get 987654321".to_owned(),
            "list 987654321 0".to_owned(),
            "list 22 0".to_owned(),
        ]
    );
    // A folder in the account's own Box needs no shared link, so none is sent.
    assert!(host.box_apis().iter().all(Option::is_none));
}

/// Box answers a wide folder a page at a time and says how many rows it holds. Every page is
/// asked for, and the walk is not over until Box stops saying there are more.
#[tokio::test]
async fn a_folder_box_answers_a_page_at_a_time_is_followed_to_the_end() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![(
        "987654321",
        "Long",
        vec![
            vec![file_row("11", "a.mkv", 1)],
            vec![file_row("12", "b.mkv", 2)],
            vec![file_row("13", "c.mkv", 3)],
        ],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        host.requests(),
        vec![
            "get 987654321".to_owned(),
            "list 987654321 0".to_owned(),
            "list 987654321 1".to_owned(),
            "list 987654321 2".to_owned(),
        ]
    );
}

/// A collection that paginates for as long as anybody asks is stopped by the crawler's own cap,
/// not by the sandbox running out of fuel.
#[tokio::test]
async fn a_folder_that_pages_further_than_the_cap_stops_at_the_cap() {
    let bytes = crawler_component();
    let pages: Vec<Vec<String>> = (0..14)
        .map(|index| {
            vec![file_row(
                &format!("{}", 100 + index),
                &format!("f{index}.bin"),
                1,
            )]
        })
        .collect();
    let host = MockBox::endless(vec![("987654321", "Endless", pages)]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");

    // Ten pages of one row each, and no eleventh request.
    assert_eq!(links.len(), 10, "{links:?}");
    let listings = host
        .requests()
        .into_iter()
        .filter(|entry| entry.starts_with("list "))
        .count();
    assert_eq!(listings, 10);
}

/// A shared link does not say what it points at. When it turns out to be one file, that one
/// file is the answer — addressed through the link it was found behind, because the link is
/// what grants the access.
#[tokio::test]
async fn a_shared_link_that_turns_out_to_be_one_file_yields_that_file() {
    let bytes = crawler_component();
    let host = MockBox::single_file();
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARED_LINK, None)
        .await
        .expect("call")
        .expect("one file");

    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("release.bin"));
    assert_eq!(links[0].size, Some(1_048_576));
    assert_eq!(links[0].package_hint, None);
    assert_eq!(links[0].url, format!("{SHARED_LINK}/file/42"));
    assert_eq!(host.requests(), vec!["shared_items".to_owned()]);
}

/// A shared link's password reaches the `boxapi` header on every request of the crawl, and
/// nothing else anywhere: not the address, not a query parameter, not what is handed back.
#[tokio::test]
async fn a_shared_links_password_travels_in_the_box_api_header_and_in_nothing_else() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![(
        "987654321",
        "Show",
        vec![vec![file_row("11", "e01.mkv", 1024)]],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARED_LINK_WITH_PASSWORD, None)
        .await
        .expect("call")
        .expect("a listing");

    let expected = format!("shared_link={SHARED_LINK}&shared_link_password={PASSWORD}");
    let sent = host.box_apis();
    assert!(!sent.is_empty());
    for value in &sent {
        assert_eq!(value.as_deref(), Some(expected.as_str()));
    }
    // Every request went to Box's own API and carried the password nowhere but that header.
    for entry in host.requests() {
        assert!(!entry.contains(PASSWORD), "{entry}");
    }
    // The one address it does travel in is the hand-over to the sibling resolver, because
    // nothing else passes between the two packages — the same reluctant exception Dropbox
    // makes (RD-106-06). `rd_core::REDACTED_QUERY` names the parameter, so it is struck out of
    // every log line, and it never reaches the address the bytes come from.
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].url,
        format!("{SHARED_LINK}/file/11?shared_link_password={PASSWORD}")
    );
    assert!(
        rd_core::is_secret_parameter("shared_link_password"),
        "the parameter a crawled address carries has to be one the core redacts"
    );
    let redacted = rd_core::redact_url(&links[0].url.parse().expect("URL"));
    assert!(!redacted.contains(PASSWORD), "{redacted}");
}

/// A folder that contains itself is read once.
#[tokio::test]
async fn a_folder_that_contains_itself_is_read_once() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![
        (
            "987654321",
            "Show",
            vec![vec![folder_row("22", "Inner"), file_row("11", "a.bin", 1)]],
        ),
        (
            "22",
            "Inner",
            vec![vec![
                folder_row("987654321", "Show"),
                folder_row("22", "Inner"),
                file_row("12", "b.bin", 2),
            ]],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    let listings = host
        .requests()
        .into_iter()
        .filter(|entry| entry.starts_with("list "))
        .count();
    assert_eq!(listings, 2, "{:?}", host.requests());
}

/// An empty folder is a refusal with a code, never a package with nothing in it.
#[tokio::test]
async fn an_empty_folder_is_a_refusal_with_a_code_and_never_an_empty_package() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![("987654321", "Empty", vec![Vec::new()])]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect_err("an empty folder is not a result");
    assert_eq!(refusal.code.as_deref(), Some("box_crawler.folder_empty"));
}

/// Denied, gone, throttled and a refused token are four different codes, because a person acts
/// differently on each — and nothing Box wrote travels into any of them.
#[tokio::test]
async fn the_ways_box_refuses_a_folder_are_four_different_codes() {
    let bytes = crawler_component();
    let cases = [
        (403, ERROR_FORBIDDEN, "box_crawler.access_denied"),
        (404, ERROR_NOT_FOUND, "box_crawler.folder_unreachable"),
        (429, ERROR_RATE_LIMIT, "box_crawler.rate_limited"),
        (401, ERROR_UNAUTHORIZED, "box_crawler.sign_in_required"),
    ];
    for (status, body, expected) in cases {
        let refusal = crawler(MockBox::failing(status, body), &bytes)
            .crawl(FOLDER, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(expected), "{status}");
        assert!(
            !refusal.message.contains("insufficient permission"),
            "{}",
            refusal.message
        );
        assert!(
            !refusal.message.contains("redacted-request-id"),
            "{}",
            refusal.message
        );
    }
    // A throttled crawl is still this plugin's folder: `not_mine` is what the link intake
    // falls back on when a crawler was wrong to claim an address, and a rate limit says
    // nothing of the sort.
    let throttled = crawler(MockBox::failing(429, ERROR_RATE_LIMIT), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect_err("a rate limit");
    assert!(!throttled.not_mine, "{throttled:?}");
}

// -- the two siblings together ----------------------------------------------------------------

/// Shared and account content use the same contract, which is the fourth acceptance criterion.
///
/// Whichever way a file was found, the crawler hands back an address the *resolver* claims, and
/// the resolver turns it into the same thing: the stable API route, pinned to the version Box
/// named, with the shared link — and only the shared link — carried in a `boxapi` header when
/// there is one. Two ways in, one download.
#[tokio::test]
async fn shared_and_account_content_use_the_same_contract() {
    let crawler_bytes = crawler_component();
    let resolver_bytes = resolver_component();

    let folders = || {
        vec![(
            "987654321",
            "Show",
            vec![vec![file_row("42", "release.bin", 1_048_576)]],
        )]
    };
    let from_account = crawler(MockBox::new(folders()), &crawler_bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");
    let from_share = crawler(MockBox::new(folders()), &crawler_bytes)
        .crawl(SHARED_LINK_WITH_PASSWORD, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(from_account[0].url, "https://app.box.com/file/42");
    assert_eq!(
        from_share[0].url,
        format!("{SHARED_LINK}/file/42?shared_link_password={PASSWORD}")
    );

    for (link, expected_box_api) in [
        (&from_account[0], None),
        (
            &from_share[0],
            Some(format!(
                "shared_link={SHARED_LINK}&shared_link_password={PASSWORD}"
            )),
        ),
    ] {
        let host = MockBox::new(Vec::new());
        let resolver = resolver(Arc::clone(&host), &resolver_bytes);
        // The sibling's answer is an address this resolver claims. That is the whole of what
        // passes between the two packages.
        assert!(
            resolver.guest_claims(&link.url).await.expect("claims"),
            "{}",
            link.url
        );
        let resolved = resolver
            .resolve(ResolveRequest {
                url: link.url.parse().expect("URL"),
                client: ClientIdentity {
                    account_id: Some(AccountId::new()),
                    proxy_profile_id: None,
                    tls_revision: 7,
                },
            })
            .await
            .expect("resolved");
        // One download route, and one version pin, whichever way the file was found.
        assert_eq!(
            resolved.url.as_str(),
            "https://api.box.com/2.0/files/42/content?version=98765"
        );
        assert_eq!(
            resolved
                .checksum
                .as_ref()
                .map(|checksum| checksum.algorithm),
            Some(rd_core::ChecksumAlgorithm::Sha1)
        );
        let sent = resolved
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("boxapi"))
            .map(|header| header.value.clone());
        assert_eq!(sent, expected_box_api);
        // The password is never in the address the bytes come from.
        assert!(!resolved.url.as_str().contains(PASSWORD));
        // And the token left the resolver as a marker, exactly as it left the crawler.
        assert_eq!(
            host.authorizations.lock().expect("authorizations").clone(),
            vec!["Bearer {{secret:box_access_token}}".to_owned()]
        );
    }
}

// -- the sign-in ------------------------------------------------------------------------------

/// What the mock token endpoint should answer next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    Granted,
    RenewalGranted,
    Denied,
    InvalidGrant,
    InvalidClient,
    RateLimited,
    Unreachable,
}

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

/// The mock Box token endpoint.
struct MockBoxAuth {
    case: Mutex<Case>,
    /// Whether the account carries the client secret of a registered application.
    registered: bool,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockBoxAuth {
    fn new(case: Case) -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(case),
            registered: true,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn unregistered() -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(Case::Granted),
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
impl ResolverHost for MockBoxAuth {
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
        let body = |status: u16, text: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers,
                body: text.as_bytes().to_vec(),
            })
        };
        match *self.case.lock().expect("case") {
            Case::Granted => body(200, TOKEN_SUCCESS, Vec::new()),
            Case::RenewalGranted => body(200, TOKEN_RENEWED, Vec::new()),
            Case::Denied => body(400, TOKEN_DENIED, Vec::new()),
            Case::InvalidGrant => body(400, TOKEN_INVALID_GRANT, Vec::new()),
            Case::InvalidClient => body(401, TOKEN_INVALID_CLIENT, Vec::new()),
            Case::RateLimited => body(
                429,
                TOKEN_RATE_LIMITED,
                vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
            ),
            Case::Unreachable => Err(Failure::coded(
                FailureKind::Offline,
                "plugin.offline",
                "box could not be reached",
            )),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.registered && reference == "box_client_secret"
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

fn parameter(request: &AuthorizationRequest, name: &str) -> Option<String> {
    url::Url::parse(&request.authorization_url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn provider(host: Arc<MockBoxAuth>, bytes: &[u8]) -> OAuthProvider {
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

const TOKEN_ENDPOINT: &str = "https://api.box.com/oauth2/token";

/// The browser sign-in, from the address the person is sent to all the way to the stored token —
/// with neither half of the application's registration anywhere in the plugin.
#[tokio::test]
async fn the_browser_sign_in_runs_through_to_a_stored_token_with_its_renewal_material() {
    let bytes = oauth_component();
    let server = MockBoxAuth::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    assert!(
        request
            .authorization_url
            .starts_with("https://account.box.com/api/oauth2/authorize?"),
        "{}",
        request.authorization_url
    );
    assert_eq!(
        parameter(&request, "response_type").as_deref(),
        Some("code")
    );
    // No client id anywhere in this plugin: the marker travels and the host substitutes what
    // *this installation* registered (RD-106-04, rule 8).
    assert!(
        request
            .authorization_url
            .contains("client_id=%7B%7Bclient_id%7D%7D")
            || request
                .authorization_url
                .contains("client_id={{client_id}}"),
        "the authorization URL must carry the marker, not a client id: {}",
        request.authorization_url
    );
    // And the address the person is sent to is one the manifest declares, which is the gate the
    // host applies before showing it.
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    assert!(rd_plugin_host::domain_allowed(
        &url::Url::parse(&request.authorization_url).expect("URL"),
        manifest.domains()
    ));
    assert_eq!(
        parameter(&request, "state").as_deref(),
        Some(request.state.as_str())
    );
    // Box takes no PKCE challenge, so there is nothing to carry from `begin` to `poll`.
    assert_eq!(request.flow_state, None);

    let outcome = plugin
        .poll(account, "the-code-the-callback-carried", None)
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, TOKEN_ENDPOINT);
    assert_eq!(exchange.field("grant_type"), Some("authorization_code"));
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    // The one thing Box needs that the other three cloud drives do not, and it is a marker too.
    assert_eq!(
        exchange.field("client_secret"),
        Some("{{secret:box_client_secret}}")
    );

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(4245),
        }]
    );
}

/// Two flows started in the same moment, for the same account, share nothing.
#[tokio::test]
async fn two_flows_in_the_same_moment_share_no_state() {
    let bytes = oauth_component();
    let plugin = provider(MockBoxAuth::new(Case::Granted), &bytes);
    let account = AccountId::new();

    let first = plugin.begin(account, None).await.expect("begin");
    let second = plugin.begin(account, None).await.expect("begin");
    assert_ne!(first.state, second.state, "the state repeated");
    for request in [&first, &second] {
        assert!(!request.state.contains(&account.to_string()));
    }
}

/// An account with no registered application is told so before anybody is sent anywhere.
#[tokio::test]
async fn a_sign_in_without_a_registered_application_reaches_nothing() {
    let bytes = oauth_component();
    let server = MockBoxAuth::unregistered();
    let plugin = provider(server.clone(), &bytes);
    let refusal = plugin
        .begin(AccountId::new(), None)
        .await
        .expect_err("no application")
        .to_string();
    assert!(
        refusal.contains("no registered Box application"),
        "{refusal}"
    );
    assert!(server.requests().is_empty(), "nothing may be asked");
}

/// A renewal sends both halves of the registration as markers, stores the new token without
/// anybody being asked, and never carries a code.
#[tokio::test]
async fn a_renewal_sends_both_credentials_as_markers_and_never_as_values() {
    let bytes = oauth_component();
    let server = MockBoxAuth::new(Case::RenewalGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .refresh(account, Some("box_access_token"))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.url, TOKEN_ENDPOINT);
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some("{{secret:box_access_token}}")
    );
    assert_eq!(sent.field("client_id"), Some("{{client_id}}"));
    assert_eq!(
        sent.field("client_secret"),
        Some("{{secret:box_client_secret}}")
    );
    assert!(sent.field("code").is_none(), "a renewal carries no code");
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(4245),
        }]
    );
}

/// Each refusal ends the flow with its own code, a provider that cannot be reached ends nothing,
/// and Box's prose reaches no message.
#[tokio::test]
async fn the_refusals_box_makes_are_told_apart_and_an_outage_is_not_one_of_them() {
    let bytes = oauth_component();
    let account = AccountId::new();
    for (case, expected) in [
        (Case::Denied, "access_denied"),
        (Case::InvalidGrant, "invalid_grant"),
        (Case::InvalidClient, "invalid_client"),
    ] {
        let server = MockBoxAuth::new(case);
        let outcome = provider(server.clone(), &bytes)
            .refresh(account, Some("box_access_token"))
            .await
            .expect("refresh");
        let TokenOutcome::Failed { category, message } = outcome else {
            panic!("{case:?} should have failed");
        };
        // Box's own error code survives, because it is shaped like one; the sentence beside it
        // does not, and neither does anything that sentence happened to quote.
        assert!(message.contains(expected), "{case:?}: {message}");
        assert!(
            !message.contains("Refresh token has expired")
                && !message.contains("denied access to your application")
                && !message.contains("client credentials are invalid"),
            "box's prose reached the message: {message}"
        );
        // A refused credential, not an outage: the renewal sweep gives this token up rather
        // than holding it (RD-106-02).
        assert_eq!(category, FailureKind::AuthRequired, "{case:?}");
        assert!(server.stored().is_empty());
    }

    let offline = MockBoxAuth::new(Case::Unreachable);
    let outcome = provider(offline.clone(), &bytes)
        .refresh(account, Some("box_access_token"))
        .await;
    assert!(
        outcome.is_err(),
        "an unreachable provider must not end the flow"
    );
    assert!(offline.stored().is_empty());
}

/// A rate limit is a wait, not a failure, and carries Box's own `Retry-After`.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = oauth_component();
    let server = MockBoxAuth::new(Case::RateLimited);
    let renewed = provider(server.clone(), &bytes)
        .refresh(AccountId::new(), Some("box_access_token"))
        .await
        .expect("refresh");
    assert_eq!(
        renewed,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );
    assert!(server.stored().is_empty(), "a rate limit stores nothing");
}

/// Box has no device entrance, and being asked for one is a refusal with a stable code rather
/// than a trap.
#[tokio::test]
async fn box_offers_no_device_sign_in_and_says_so() {
    let bytes = oauth_component();
    let server = MockBoxAuth::new(Case::Granted);
    let refusal = provider(server.clone(), &bytes)
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("no device flow")
        .to_string();
    assert!(refusal.contains("device code"), "{refusal}");
    assert!(server.requests().is_empty());
}

// -- the shape of the three packages -----------------------------------------------------------

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/box");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        // `code` is deliberately not in this list: in a Box *error* document it is the API's
        // own error name, which is exactly what is meant to survive. The values that must not
        // are the two tokens and the request id Box stamps every refusal with.
        for field in ["access_token", "refresh_token", "request_id"] {
            let Some(found) = value.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            assert!(
                found.starts_with("redacted-"),
                "{path:?} carries a `{field}` that is not a placeholder"
            );
        }
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures were checked");
}

/// The three siblings reach only Box, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying the
/// provider row, one crawler and one sign-in claiming that row's slug, and no capability beyond
/// HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_box_each_one_needs() {
    let resolver: PluginManifest = toml::from_str(RESOLVER_MANIFEST).expect("resolver manifest");
    let crawler: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("crawler manifest");
    let sign_in: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("oauth manifest");

    // Only a resolver manifest may carry a provider row, so the account, its vault references
    // and the slug the other two claim all live in exactly one of the three.
    let provider = resolver.provider.as_ref().expect("the provider row");
    assert_eq!(provider.slug, "box");
    assert_eq!(
        provider.credentials,
        rd_plugin_host::CredentialKindManifest::OAuth
    );
    // The account's username field holds this installation's own client id, and it is required —
    // a sign-in without one cannot start (RD-106-04, rule 8).
    assert!(
        provider.username_required,
        "an OAuth provider whose client is registered per installation has to ask for it"
    );
    // Two slots, because Box's token endpoint needs a client secret on every grant: the one the
    // person fills, and the one the sign-in fills (RD-106-03). Writing the second over the first
    // would destroy the value every later renewal needs.
    let slots = provider.secret_slots();
    assert_eq!(slots.len(), 2, "{slots:?}");
    let person = slots
        .iter()
        .find(|slot| slot.reference == "box_client_secret")
        .expect("the slot the person fills");
    let flow = slots
        .iter()
        .find(|slot| slot.reference == "box_access_token")
        .expect("the slot the sign-in fills");
    assert_eq!(person.filled_by, SecretFilledByManifest::Person);
    assert_eq!(flow.filled_by, SecretFilledByManifest::Flow);
    // The exact host the token may be sent to, and the one the download engine checks the
    // transfer address against before attaching the bearer header.
    assert_eq!(flow.domains, vec!["api.box.com".to_owned()]);
    assert!(crawler.provider.is_none() && sign_in.provider.is_none());

    for manifest in [&crawler, &sign_in] {
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec!["box".to_owned()], "{}", manifest.name);
    }

    // The Content API for the two that read items; Box's own sign-in hosts for the third.
    assert_eq!(
        resolver.capabilities.domains(),
        ["api.box.com".to_owned()].as_slice()
    );
    assert_eq!(
        crawler.capabilities.domains(),
        ["api.box.com".to_owned()].as_slice()
    );
    assert_eq!(
        sign_in.capabilities.domains(),
        ["account.box.com".to_owned(), "api.box.com".to_owned()].as_slice()
    );
    // The resolver spends the access token; the sign-in holds the client secret. Neither is
    // granted the other's, which is what least privilege means with two credentials in play.
    assert_eq!(
        resolver.capabilities.secrets,
        vec!["box_access_token".to_owned()]
    );
    assert_eq!(
        sign_in.capabilities.secrets,
        vec!["box_client_secret".to_owned()]
    );

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

    // One way in, and stated rather than guessed at: Box has no device entrance.
    assert_eq!(
        sign_in.oauth_flows(),
        [OAuthFlowManifest::Redirect].as_slice()
    );
    assert!(!sign_in.serves_oauth_flow(OAuthFlowManifest::Device));

    // The bytes come from the API route and the storage host it redirects to, and from nowhere
    // else — never `*`, because this is a hoster for its own cloud.
    assert_eq!(
        resolver.download_domains,
        vec!["api.box.com".to_owned(), "*.boxcloud.com".to_owned()]
    );
    assert!(!resolver.match_domains.iter().any(|domain| domain == "*"));
}

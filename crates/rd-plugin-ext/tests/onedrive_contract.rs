//! OneDrive and SharePoint, exercised end to end against a mock Microsoft Graph and a mock
//! Microsoft identity platform (RD-106-05).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither graph.microsoft.com nor login.microsoftonline.com is contacted. A run against a real
//! Microsoft account is *not* claimed here; there is none in this checkout, and there is no
//! registered application either. What is proven is everything that does not need one.
//!
//! Two of the three OneDrive plugins are driven here as real WebAssembly components, because
//! that is the only place their guest code exists: the crawler's paging walk and the sign-in's
//! two entrances are written against the WIT imports and cannot run natively. The third,
//! `plugins/onedrive/`, keeps its protocol logic outside the component and is covered by
//! `plugins/onedrive/src/native/tests.rs` against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A shared folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Graph answers a page at a time | every `@odata.nextLink` followed, to the cap |
//! | A long address that turns out to be one file | that one file |
//! | A folder that contains itself | read once |
//! | An empty folder | a refusal with a code, never an empty package |
//! | A next page that leaves Graph | refused before it is fetched |
//! | Denied, not found, throttled, refused token | four different codes |
//! | The browser sign-in | PKCE through to a stored token and its renewal material |
//! | The device sign-in | a code to type, a wait, a stored token, the same renewal |
//! | The application id | a marker in the address and in every exchange, never a value |

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    OAuthFlowManifest, PluginManifest,
    extension::{AuthorizationRequest, FolderCrawler, OAuthProvider, TokenOutcome},
};

const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/onedrive-crawler/manifest.toml");
const OAUTH_MANIFEST: &str = include_str!("../../../plugins/onedrive-oauth/manifest.toml");
const RESOLVER_MANIFEST: &str = include_str!("../../../plugins/onedrive/manifest.toml");

// The sanitised fixtures. Every token-shaped value in them is a placeholder and nothing else;
// `fixtures_carry_no_credential_material` is what keeps it that way.
const TOKEN_SUCCESS: &str = include_str!("fixtures/onedrive/token_success.json");
const TOKEN_RENEWED: &str = include_str!("fixtures/onedrive/token_renewed.json");
const TOKEN_DENIED: &str = include_str!("fixtures/onedrive/token_denied.json");
const TOKEN_INVALID_GRANT: &str = include_str!("fixtures/onedrive/token_invalid_grant.json");
const TOKEN_RATE_LIMITED: &str = include_str!("fixtures/onedrive/token_rate_limited.json");
const DEVICE_CODE: &str = include_str!("fixtures/onedrive/device_code.json");
const DEVICE_PENDING: &str = include_str!("fixtures/onedrive/device_pending.json");
const DEVICE_DECLINED: &str = include_str!("fixtures/onedrive/device_declined.json");
const ITEM_ACCESS_DENIED: &str = include_str!("fixtures/onedrive/item_access_denied.json");
const ITEM_NOT_FOUND: &str = include_str!("fixtures/onedrive/item_not_found.json");
const ITEM_INVALID_REQUEST: &str = include_str!("fixtures/onedrive/item_invalid_request.json");
const ITEM_RATE_LIMITED: &str = include_str!("fixtures/onedrive/item_rate_limited.json");
const ITEM_UNAUTHENTICATED: &str = include_str!("fixtures/onedrive/item_unauthenticated.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";
const PLACEHOLDER_DEVICE_CODE: &str = "redacted-device-code-0000";

/// A folder sharing link, as OneDrive hands them out.
const FOLDER_LINK: &str = "https://1drv.ms/f/s!AkXy_Zabc-DEF";
/// The long personal address, which says nothing about what it points at.
const UNDECIDED_LINK: &str = "https://onedrive.live.com/?id=ABC123%21456&cid=ABC123";

/// A bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn crawler_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-onedrive-crawler")
}

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-onedrive-oauth")
}

/// The share id Graph uses for a sharing link, computed independently of the plugin so a
/// matching request is evidence rather than a tautology.
fn share_of(link: &str) -> String {
    format!(
        "u!{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(link.as_bytes())
    )
}

// -- the mock Graph API ---------------------------------------------------------------------

/// One `/children` answer, as one page of one folder.
type Pages = Vec<String>;

/// What the shared root is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Folder,
    File,
}

/// The mock Graph API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:onedrive_access_token}}` and never as a value.
struct MockGraph {
    root: Root,
    /// `(item id — empty for the shared root — its display name, its pages)`.
    folders: Vec<(&'static str, &'static str, Pages)>,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    /// When set, every request fails with this status and this body instead of being answered.
    failure: Option<(u16, &'static str)>,
}

impl MockGraph {
    fn new(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            root: Root::Folder,
            folders,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    fn single_file() -> Arc<Self> {
        Arc::new(Self {
            root: Root::File,
            folders: Vec::new(),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    fn failing(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            root: Root::Folder,
            folders: Vec::new(),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: Some((status, body)),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

/// One `/children` page. `next` becomes the `@odata.nextLink` Graph would send: the same
/// address with a `$skiptoken`, which is how the mock tells the pages apart.
fn page(entries: &[String], next: Option<&str>) -> String {
    let link = next.map_or(String::new(), |token| {
        format!(
            r#""@odata.nextLink":"https://graph.microsoft.com/v1.0/shares/x/driveItem/children?$skiptoken={token}","#
        )
    });
    format!(r#"{{{link}"value":[{}]}}"#, entries.join(","))
}

/// A page whose next link points away from Graph.
fn page_leaving_graph(entries: &[String]) -> String {
    format!(
        r#"{{"@odata.nextLink":"https://evil.test/children?$skiptoken=x","value":[{}]}}"#,
        entries.join(",")
    )
}

fn file(id: &str, name: &str, size: u64) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","size":{size},"file":{{"mimeType":"video/x-matroska"}}}}"#
    )
}

fn subfolder(id: &str, name: &str) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","folder":{{"childCount":1}}}}"#)
}

#[async_trait]
impl ResolverHost for MockGraph {
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
        // `/shares/{s}/driveItem[/children]` for the root, `/shares/{s}/items/{id}[/children]`
        // below it. Told apart by the route, the way Graph tells them apart, rather than by the
        // order the plugin happens to make them in.
        let segments: Vec<&str> = request
            .url
            .path_segments()
            .map(|segments| segments.collect())
            .unwrap_or_default();
        let (folder, listing) = match segments.as_slice() {
            ["v1.0", "shares", _, "driveItem"] => ("", false),
            ["v1.0", "shares", _, "driveItem", "children"] => ("", true),
            ["v1.0", "shares", _, "items", id] => (*id, false),
            ["v1.0", "shares", _, "items", id, "children"] => (*id, true),
            _ => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("unexpected {}", request.url.path()));
                return answer(404, ITEM_NOT_FOUND.to_owned());
            }
        };
        // The next-page token rides in the URL, because the plugin follows Graph's own
        // `@odata.nextLink` as given; the first page's `$select` and `$top` ride as templates.
        let token = request
            .url
            .query_pairs()
            .find(|(name, _)| name == "$skiptoken")
            .map(|(_, value)| value.into_owned());
        if listing {
            self.requests.lock().expect("requests").push(format!(
                "list {folder} {}",
                token.clone().unwrap_or_default()
            ));
            let index = token
                .and_then(|token| token.strip_prefix("page-").map(str::to_owned))
                .and_then(|number| number.parse::<usize>().ok())
                .unwrap_or(0);
            let pages = self
                .folders
                .iter()
                .find(|(id, _, _)| *id == folder)
                .map(|(_, _, pages)| pages);
            return match pages.and_then(|pages| pages.get(index)) {
                Some(body) => answer(200, body.clone()),
                None => answer(200, page(&[], None)),
            };
        }
        self.requests
            .lock()
            .expect("requests")
            .push(format!("get {folder}"));
        if folder.is_empty() && self.root == Root::File {
            return answer(
                200,
                r#"{"id":"redacted-item-id-0001","name":"release.bin","size":1048576,"file":{}}"#
                    .to_owned(),
            );
        }
        match self.folders.iter().find(|(known, _, _)| *known == folder) {
            Some((id, name, _)) => answer(
                200,
                format!(
                    r#"{{"id":"{}","name":"{name}","folder":{{"childCount":1}}}}"#,
                    if id.is_empty() { "root-id" } else { id }
                ),
            ),
            None => answer(404, ITEM_NOT_FOUND.to_owned()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn crawler(host: Arc<MockGraph>, bytes: &[u8]) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the crawler manifest");
    FolderCrawler::new(manifest, bytes, Some(host)).expect("compile the crawler")
}

// -- the crawler ----------------------------------------------------------------------------

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// The check that keeps a crawler from costing every other link in the queue a request — and,
/// here, the one that keeps the two siblings apart: a file link is the resolver's, and the
/// canonical Graph address the crawler itself hands back is never listed again.
#[tokio::test]
async fn only_onedrive_folder_links_and_undecided_ones_are_claimed() {
    let bytes = crawler_component();
    let host = MockGraph::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        FOLDER_LINK,
        "https://1drv.ms/f/c/ab12cd34ef56/EaBcDeFgHiJ?e=abc",
        "https://contoso-my.sharepoint.com/:f:/g/personal/someone_contoso_com/EaBc?e=x",
        "https://contoso.sharepoint.com/:f:/s/Team/EaBcDeF",
        UNDECIDED_LINK,
        "https://onedrive.live.com/redir?resid=ABC123%21456&authkey=%21AB",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // The sibling resolver's addresses, every spelling of them.
        "https://1drv.ms/u/s!AkXy_Zabc-DEF",
        "https://contoso.sharepoint.com/:b:/s/Team/EaBcDeF",
        "https://onedrive.live.com/view.aspx?resid=ABC123%21456",
        "https://graph.microsoft.com/v1.0/shares/u!aHR0/items/01ABC",
        "https://graph.microsoft.com/v1.0/shares/u!aHR0/driveItem",
        // And somebody else's entirely.
        "https://1drv.ms.evil.test/f/s!abc",
        "https://ddownload.com/f/abc",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.requests().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A shared folder comes back as its files, with the names, the sizes and the structure Graph
/// gave — and each as the canonical address the sibling resolver claims, carrying the share
/// it was found through.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![
        (
            "",
            "Show",
            vec![page(
                &[file("x0", "readme.txt", 12), subfolder("s1", "Season 1")],
                None,
            )],
        ),
        (
            "s1",
            "Season 1",
            vec![page(
                &[file("x1", "e01.mkv", 1024), file("x2", "e02.mkv", 2048)],
                None,
            )],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
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
    // The one thing that crosses between the two packages: an address, and nothing else —
    // the item inside the share it was found through, never the item on its own.
    let share = share_of(FOLDER_LINK);
    assert_eq!(
        links[1].url,
        format!("https://graph.microsoft.com/v1.0/shares/{share}/items/x1")
    );
    assert_eq!(
        host.requests(),
        vec![
            "get ".to_owned(),
            "list  ".to_owned(),
            "list s1 ".to_owned()
        ]
    );
    // The sharing link went to Graph whole, as the share id Microsoft documents.
    let authorizations = host.authorizations.lock().expect("authorizations");
    assert_eq!(authorizations.len(), 3);
}

/// Graph answers a wide folder a page at a time and names the next page in
/// `@odata.nextLink`. Every page is followed, as given, and the walk is not over until Graph
/// stops naming one.
#[tokio::test]
async fn a_folder_graph_answers_a_page_at_a_time_is_followed_to_the_end() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![(
        "",
        "Long",
        vec![
            page(&[file("x1", "a.mkv", 1)], Some("page-1")),
            page(&[file("x2", "b.mkv", 2)], Some("page-2")),
            page(&[file("x3", "c.mkv", 3)], None),
        ],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        host.requests(),
        vec![
            "get ".to_owned(),
            "list  ".to_owned(),
            "list  page-1".to_owned(),
            "list  page-2".to_owned(),
        ],
        "the next link has to be followed as given, or the walk repeats page one"
    );
}

/// A next page that leaves Graph is refused before it is fetched. The sandbox would refuse it
/// too, but a refusal with a name beats one without.
#[tokio::test]
async fn a_next_page_that_leaves_graph_is_refused_before_it_is_fetched() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![(
        "",
        "Trap",
        vec![page_leaving_graph(&[file("x1", "a.mkv", 1)])],
    )]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("a next link off Graph is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("onedrive_crawler.invalid_response")
    );
    assert!(
        !host.requests().iter().any(|entry| entry.contains("evil")),
        "{:?}",
        host.requests()
    );
}

/// The long personal address does not say what it points at. When it turns out to be one
/// file, that one file is the answer — with the share as its address, because the resolver
/// never sees the pasted link.
#[tokio::test]
async fn an_address_that_does_not_say_and_points_at_a_file_yields_that_file() {
    let bytes = crawler_component();
    let host = MockGraph::single_file();
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(UNDECIDED_LINK, None)
        .await
        .expect("call")
        .expect("one file");
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("release.bin"));
    assert_eq!(links[0].size, Some(1_048_576));
    assert_eq!(links[0].package_hint, None);
    assert_eq!(
        links[0].url,
        format!(
            "https://graph.microsoft.com/v1.0/shares/{}/driveItem",
            share_of(UNDECIDED_LINK)
        )
    );
    assert_eq!(
        host.requests(),
        vec!["get ".to_owned()],
        "one request, no listing"
    );

    // A link that *said* it was a folder and is not one is refused by name instead.
    let refusal = crawler(MockGraph::single_file(), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("a folder link that is a file is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("onedrive_crawler.not_a_folder")
    );
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![(
        "",
        "Show",
        vec![page(&[file("x1", "a.mkv", 1)], None)],
    )]);
    crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect("a listing");

    let authorizations = host.authorizations.lock().expect("authorizations").clone();
    assert!(!authorizations.is_empty());
    for value in authorizations {
        assert_eq!(
            value, "Bearer {{secret:onedrive_access_token}}",
            "the plugin must name the reference, never hold the token"
        );
    }
}

/// A folder that contains itself is read once, not for ever.
#[tokio::test]
async fn a_cycle_is_walked_once() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![
        (
            "",
            "Loop",
            vec![page(
                &[subfolder("inner", "Inner"), file("x1", "a.mkv", 1)],
                None,
            )],
        ),
        (
            "inner",
            "Inner",
            vec![page(
                &[
                    subfolder("root-id", "Back"),
                    subfolder("inner", "Self"),
                    file("x2", "b.mkv", 2),
                ],
                None,
            )],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(
        host.requests()
            .iter()
            .filter(|entry| entry.starts_with("list "))
            .count(),
        2,
        "each folder is listed exactly once"
    );
}

/// An empty folder says so, with a code the interface can translate.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![("", "Empty", vec![page(&[], None)])]);
    let refusal = crawler(host, &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("onedrive_crawler.folder_empty")
    );
}

/// The ways a crawl can fail reach the person as different codes, and none of them repeats a
/// word Microsoft wrote. (That a throttle carries Microsoft's own `Retry-After` is proven at the
/// resolver, in `plugins/onedrive/src/native/tests.rs`; a crawl refusal carries no category.)
#[tokio::test]
async fn a_refused_token_a_throttle_a_denied_share_and_a_missing_folder_are_told_apart() {
    let bytes = crawler_component();
    for (status, body, expected) in [
        (
            401,
            ITEM_UNAUTHENTICATED,
            "onedrive_crawler.sign_in_required",
        ),
        (429, ITEM_RATE_LIMITED, "onedrive_crawler.rate_limited"),
        (403, ITEM_ACCESS_DENIED, "onedrive_crawler.access_denied"),
        (404, ITEM_NOT_FOUND, "onedrive_crawler.folder_unreachable"),
        (
            400,
            ITEM_INVALID_REQUEST,
            "onedrive_crawler.folder_unreachable",
        ),
    ] {
        let refusal = crawler(MockGraph::failing(status, body), &bytes)
            .crawl(FOLDER_LINK, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(expected), "{status}");
        assert!(
            !refusal.message.contains("Access denied")
                && !refusal.message.contains("throttled")
                && !refusal.message.contains("could not be found"),
            "a provider's prose reached the message: {}",
            refusal.message
        );
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
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

// -- the sign-in ----------------------------------------------------------------------------

/// What Microsoft's endpoints should answer next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    /// Checks the PKCE verifier and, when it matches, hands the tokens over.
    Granted,
    /// A renewal: no PKCE, and the rotated refresh material Microsoft sends with every one.
    RenewalGranted,
    Denied,
    InvalidGrant,
    RateLimited,
    /// Microsoft could not be reached at all — not an answer, a failed call.
    Unreachable,
    /// The device endpoint hands out a code and the token endpoint says "not yet".
    DevicePending,
    /// The device endpoint hands out a code and the token endpoint grants.
    DeviceGranted,
    /// The device endpoint hands out a code and the person declines at the other screen.
    DeviceDeclined,
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

/// The mock Microsoft identity platform: one device-code endpoint and one token endpoint.
struct MockMicrosoft {
    case: Mutex<Case>,
    challenge: Mutex<Option<String>>,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockMicrosoft {
    fn new(case: Case) -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(case),
            challenge: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn set_case(&self, case: Case) {
        *self.case.lock().expect("case") = case;
    }

    fn expect_challenge(&self, challenge: &str) {
        *self.challenge.lock().expect("challenge") = Some(challenge.to_owned());
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn stored(&self) -> Vec<Stored> {
        self.stored.lock().expect("stored").clone()
    }
}

#[async_trait]
impl ResolverHost for MockMicrosoft {
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
        let url = request.url.to_string();
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            url: url.clone(),
            form: form.clone(),
        });
        let body = |status: u16, text: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers,
                body: text.as_bytes().to_vec(),
            })
        };
        let case = *self.case.lock().expect("case");
        // The device entrance is two endpoints, not one, so the mock has to tell them apart
        // the way Microsoft does: by the address.
        if url.ends_with("/devicecode") && case != Case::Unreachable {
            assert_eq!(
                form.iter()
                    .find(|(name, _)| name == "client_id")
                    .map(|(_, value)| value.as_str()),
                Some("{{client_id}}"),
                "a device code is asked for with the application marker, never a value"
            );
            return body(200, DEVICE_CODE, Vec::new());
        }
        match case {
            Case::Granted => {
                // The check a real authorization server makes: the verifier the client kept
                // back has to hash to the challenge it published.
                let expected = self.challenge.lock().expect("challenge").clone();
                let verifier = form
                    .iter()
                    .find(|(name, _)| name == "code_verifier")
                    .map(|(_, value)| value.clone());
                match (expected, verifier) {
                    (Some(expected), Some(verifier)) if s256(&verifier) == expected => {
                        body(200, TOKEN_SUCCESS, Vec::new())
                    }
                    _ => body(400, TOKEN_INVALID_GRANT, Vec::new()),
                }
            }
            Case::RenewalGranted => {
                assert!(
                    form.iter().all(|(name, _)| name != "code_verifier"),
                    "a refresh grant must not carry a PKCE verifier"
                );
                body(200, TOKEN_RENEWED, Vec::new())
            }
            Case::Denied => body(400, TOKEN_DENIED, Vec::new()),
            Case::InvalidGrant => body(400, TOKEN_INVALID_GRANT, Vec::new()),
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
                "microsoft could not be reached",
            )),
            Case::DevicePending => body(400, DEVICE_PENDING, Vec::new()),
            Case::DeviceGranted => {
                assert!(
                    form.iter().all(|(name, _)| name != "code_verifier"),
                    "a device grant carries no PKCE verifier"
                );
                body(200, TOKEN_SUCCESS, Vec::new())
            }
            Case::DeviceDeclined => body(400, DEVICE_DECLINED, Vec::new()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
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

/// base64url, unpadded, of the SHA-256 digest — PKCE's `S256`, computed independently of the
/// plugin so a matching pair is evidence rather than a tautology.
fn s256(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn parameter(request: &AuthorizationRequest, name: &str) -> Option<String> {
    url::Url::parse(&request.authorization_url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn provider(host: Arc<MockMicrosoft>, bytes: &[u8]) -> OAuthProvider {
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

const TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// Authorization code with PKCE, from the address the person is sent to all the way to the
/// stored token — asking for the least the three plugins need, and for the renewal scope.
#[tokio::test]
async fn the_browser_sign_in_runs_through_to_a_stored_token_with_its_renewal_material() {
    let bytes = oauth_component();
    let server = MockMicrosoft::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    assert!(
        request
            .authorization_url
            .starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?"),
        "{}",
        request.authorization_url
    );
    assert_eq!(
        parameter(&request, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        parameter(&request, "response_type").as_deref(),
        Some("code")
    );
    // Least privilege: `Files.Read.All` because a sharing link goes through `/shares`, which
    // Graph grants to nothing narrower; `offline_access` because without it there is no
    // refresh token and the person is asked again every hour. Nothing that can write.
    assert_eq!(
        parameter(&request, "scope").as_deref(),
        Some("Files.Read.All offline_access")
    );
    // No application id anywhere in this plugin: the marker travels and the host substitutes
    // what *this installation* registered (RD-106-04, rule 8).
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
    let challenge = parameter(&request, "code_challenge").expect("a challenge");
    let verifier = request.flow_state.clone().expect("a verifier");
    assert!(
        !request.authorization_url.contains(&verifier),
        "the verifier must never appear in the address the person is sent to"
    );
    assert_eq!(s256(&verifier), challenge);
    assert_eq!(
        parameter(&request, "state").as_deref(),
        Some(request.state.as_str())
    );
    assert!((43..=128).contains(&verifier.len()));

    server.expect_challenge(&challenge);
    let outcome = plugin
        .poll(
            account,
            "the-code-the-callback-carried",
            request.flow_state.as_deref(),
        )
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, TOKEN_ENDPOINT);
    assert_eq!(exchange.field("grant_type"), Some("authorization_code"));
    assert_eq!(exchange.field("code_verifier"), Some(verifier.as_str()));
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3599),
        }]
    );
}

/// A stolen code without its verifier buys nothing, which is the whole reason PKCE exists.
#[tokio::test]
async fn a_code_without_its_verifier_is_refused() {
    let bytes = oauth_component();
    let server = MockMicrosoft::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();
    let request = plugin.begin(account, None).await.expect("begin");
    server.expect_challenge(&parameter(&request, "code_challenge").expect("challenge"));

    let outcome = plugin
        .poll(
            account,
            "a-stolen-code",
            Some("a-verifier-from-another-flow"),
        )
        .await
        .expect("poll");
    assert!(
        matches!(outcome, TokenOutcome::Failed { .. }),
        "{outcome:?}"
    );
    assert!(server.stored().is_empty(), "nothing may be stored");
}

/// Two flows started in the same moment, for the same account, share nothing.
#[tokio::test]
async fn two_flows_in_the_same_moment_share_no_secret() {
    let bytes = oauth_component();
    let plugin = provider(MockMicrosoft::new(Case::Granted), &bytes);
    let account = AccountId::new();

    let first = plugin.begin(account, None).await.expect("begin");
    let second = plugin.begin(account, None).await.expect("begin");
    assert_ne!(first.state, second.state, "the state repeated");
    assert_ne!(first.flow_state, second.flow_state, "the verifier repeated");
    for request in [&first, &second] {
        assert!(!request.state.contains(&account.to_string()));
    }
}

/// The device entrance, end to end: a code the person types at microsoft.com/devicelogin, a
/// poll that waits, and a token that lands in the vault with the same refresh material the
/// browser sign-in produces — so the renewal sweep treats the two alike.
#[tokio::test]
async fn the_device_sign_in_runs_through_to_the_same_stored_token() {
    let bytes = oauth_component();
    let server = MockMicrosoft::new(Case::DevicePending);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    assert_eq!(prompt.verification_url, "https://microsoft.com/devicelogin");
    assert_eq!(prompt.user_code.as_deref(), Some("ABCD1234"));
    assert_eq!(prompt.expires_in_seconds, Some(900));
    assert_eq!(prompt.interval_seconds, Some(5));
    // The device code is what the poll is made with and is never part of what is shown.
    let device_code = prompt.flow_state.clone().expect("a device code");
    assert_eq!(device_code, PLACEHOLDER_DEVICE_CODE);
    assert_ne!(Some(device_code.as_str()), prompt.user_code.as_deref());
    assert!(!prompt.verification_url.contains(&device_code));
    // And the address the person is sent to is one the manifest declares, which is the gate
    // the host applies before showing it.
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    assert!(rd_plugin_host::domain_allowed(
        &url::Url::parse(&prompt.verification_url).expect("URL"),
        manifest.domains()
    ));

    // Nobody has confirmed yet. That is a wait, not a failure.
    let waiting = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(
        waiting,
        TokenOutcome::Pending {
            retry_after_seconds: 5
        }
    );
    assert!(server.stored().is_empty(), "a wait stores nothing");

    server.set_case(Case::DeviceGranted);
    let granted = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(granted, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.url, TOKEN_ENDPOINT);
    assert_eq!(
        exchange.field("grant_type"),
        Some("urn:ietf:params:oauth:grant-type:device_code")
    );
    assert_eq!(exchange.field("device_code"), Some(device_code.as_str()));
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    assert!(exchange.field("code").is_none());

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3599),
        }]
    );
}

/// A person who declines at the other screen ends the flow, with the code the interface
/// translates as "not granted" and none of the AADSTS prose beside it.
#[tokio::test]
async fn a_device_sign_in_the_person_declined_ends_the_flow() {
    let bytes = oauth_component();
    let server = MockMicrosoft::new(Case::DeviceDeclined);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    let outcome = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    let TokenOutcome::Failed { message, category } = outcome else {
        panic!("declining should have failed, got {outcome:?}");
    };
    assert_eq!(category, FailureKind::AuthRequired);
    assert!(message.contains("authorization_declined"), "{message}");
    assert!(
        !message.contains("AADSTS") && !message.contains("denied the authorization"),
        "microsoft's prose reached the message: {message}"
    );
    assert!(server.stored().is_empty());
}

/// A refusal ends the flow and an unreachable Microsoft does not — the distinction the renewal
/// sweep acts on, and the one that would otherwise sign people out whenever a connection drops.
#[tokio::test]
async fn a_refused_grant_ends_the_flow_and_an_unreachable_microsoft_does_not() {
    let bytes = oauth_component();
    let account = AccountId::new();

    for (case, expected) in [
        (Case::Denied, "access_denied"),
        (Case::InvalidGrant, "invalid_grant"),
    ] {
        let server = MockMicrosoft::new(case);
        let outcome = provider(server.clone(), &bytes)
            .refresh(account, Some("onedrive_access_token"))
            .await
            .expect("refresh");
        let TokenOutcome::Failed { message, .. } = outcome else {
            panic!("{case:?} should have failed, got {outcome:?}");
        };
        // Microsoft's own error code survives; the AADSTS sentence beside it does not.
        assert!(message.contains(expected), "{message}");
        assert!(
            !message.contains("AADSTS") && !message.contains("Trace ID"),
            "microsoft's prose reached the message: {message}"
        );
        assert!(server.stored().is_empty());
    }

    let offline = MockMicrosoft::new(Case::Unreachable);
    let outcome = provider(offline.clone(), &bytes)
        .refresh(account, Some("onedrive_access_token"))
        .await;
    assert!(
        outcome.is_err(),
        "an unreachable provider must not end the flow"
    );
    assert!(offline.stored().is_empty());
}

/// A renewal stores the new token without anybody being asked, and the stored refresh material
/// leaves the plugin as a template rather than as a value. Microsoft rotates the refresh token
/// on every renewal, and the rotated one is what gets stored.
#[tokio::test]
async fn a_renewal_stores_a_new_token_without_a_redirect_or_a_device_code() {
    let bytes = oauth_component();
    let server = MockMicrosoft::new(Case::RenewalGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .refresh(account, Some("onedrive_access_token"))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.url, TOKEN_ENDPOINT);
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some("{{secret:onedrive_access_token}}")
    );
    assert_eq!(sent.field("client_id"), Some("{{client_id}}"));
    // Microsoft's token endpoint answers a narrower token when the scope is left off a
    // renewal, so it is repeated.
    assert_eq!(sent.field("scope"), Some("Files.Read.All offline_access"));
    assert!(sent.field("code").is_none(), "a renewal carries no code");
    assert!(
        sent.field("device_code").is_none(),
        "a renewal is not a device sign-in"
    );
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3599),
        }]
    );
}

/// A rate limit is a wait, not a failure, and carries Microsoft's own `Retry-After`.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = oauth_component();
    let server = MockMicrosoft::new(Case::RateLimited);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let renewed = plugin
        .refresh(account, Some("onedrive_access_token"))
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

// -- the shape of the three packages ---------------------------------------------------------

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/onedrive");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        for field in [
            "access_token",
            "refresh_token",
            "id_token",
            "code",
            "device_code",
        ] {
            let Some(found) = value.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            assert!(
                found == PLACEHOLDER_ACCESS_TOKEN
                    || found == PLACEHOLDER_REFRESH_TOKEN
                    || found == PLACEHOLDER_DEVICE_CODE,
                "{path:?} carries a `{field}` that is not a placeholder"
            );
        }
        // The pre-authenticated download address a real driveItem carries is a credential in
        // its query string; the fixture keeps the field and redacts the value.
        if let Some(found) = value
            .get("@microsoft.graph.downloadUrl")
            .and_then(serde_json::Value::as_str)
        {
            assert!(
                found.starts_with("https://example.invalid/") && found.contains("redacted"),
                "{path:?} carries a download address that is not a placeholder"
            );
        }
        checked += 1;
    }
    assert!(checked >= 14, "only {checked} fixtures were checked");
}

/// The three siblings reach only Microsoft, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying
/// the provider row, one crawler and one sign-in claiming that row's slug, and no capability
/// beyond HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_microsoft_each_one_needs() {
    let resolver: PluginManifest = toml::from_str(RESOLVER_MANIFEST).expect("resolver manifest");
    let crawler: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("crawler manifest");
    let sign_in: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("oauth manifest");

    // Only a resolver manifest may carry a provider row, so the account, its vault reference
    // and the slug the other two claim all live in exactly one of the three.
    let provider = resolver.provider.as_ref().expect("the provider row");
    assert_eq!(provider.slug, "onedrive");
    assert_eq!(
        provider.credentials,
        rd_plugin_host::CredentialKindManifest::OAuth
    );
    assert_eq!(
        provider.secret_reference.as_deref(),
        Some("onedrive_access_token")
    );
    // The account's username field holds this installation's own application id, and it is
    // required — a sign-in without one cannot start (RD-106-04, rule 8).
    assert!(
        provider.username_required,
        "an OAuth provider whose client is registered per installation has to ask for it"
    );
    // The exact host the account's token may be sent to, and the one the download engine
    // checks the transfer address against before attaching the bearer header.
    assert_eq!(
        provider.secret_domains,
        vec!["graph.microsoft.com".to_owned()]
    );
    assert!(crawler.provider.is_none() && sign_in.provider.is_none());

    for manifest in [&crawler, &sign_in] {
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec!["onedrive".to_owned()], "{}", manifest.name);
    }

    // Graph for the two that read items; Microsoft's own sign-in hosts for the third — plus
    // the host a device code is typed at, because the verification address is gated on it.
    assert_eq!(
        resolver.capabilities.domains(),
        ["graph.microsoft.com".to_owned()].as_slice()
    );
    assert_eq!(
        crawler.capabilities.domains(),
        ["graph.microsoft.com".to_owned()].as_slice()
    );
    assert_eq!(
        sign_in.capabilities.domains(),
        [
            "login.microsoftonline.com".to_owned(),
            "microsoft.com".to_owned(),
            "www.microsoft.com".to_owned(),
        ]
        .as_slice()
    );

    // No cookies, no captcha and no raw sockets anywhere: three API clients and a sign-in.
    for manifest in [&resolver, &crawler, &sign_in] {
        assert!(!manifest.capabilities.cookies, "{}", manifest.name);
        assert!(!manifest.capabilities.captcha, "{}", manifest.name);
        assert!(
            manifest.capabilities.net_stream.is_none(),
            "{}",
            manifest.name
        );
    }
    // The sign-in holds no vault reference of its own: what it may expand is chosen per
    // invocation by the host, from the account.
    assert!(sign_in.capabilities.secrets.is_empty());

    // Both ways in, browser first, and stated rather than guessed at (RD-106-01).
    assert_eq!(
        sign_in.oauth_flows(),
        [OAuthFlowManifest::Redirect, OAuthFlowManifest::Device].as_slice()
    );
    assert!(sign_in.serves_oauth_flow(OAuthFlowManifest::Device));
}

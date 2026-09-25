//! Dropbox, exercised end to end against a mock Dropbox API and a mock Dropbox authorization
//! server (RD-106-06).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither dropboxapi.com nor dropbox.com is contacted. A run against a real Dropbox account
//! is *not* claimed here; there is none in this checkout, and there is no registered app
//! either. What is proven is everything that does not need one.
//!
//! Two of the three Dropbox plugins are driven here as real WebAssembly components, because
//! that is the only place their guest code exists: the crawler's cursor walk and the sign-in's
//! exchange are written against the WIT imports and cannot run natively. The third,
//! `plugins/dropbox/`, keeps its protocol logic outside the component and is covered by
//! `plugins/dropbox/src/native/tests.rs` against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Dropbox answers a page at a time | every cursor followed, out of the walk's state |
//! | A shared folder link with a password | listed through `shared_link`, files carry it on |
//! | The same link without its password | one code, `link_access_denied` |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Refused token, rate limit, missing folder | three different codes |
//! | The sign-in | PKCE through to a stored token and its renewal material |
//! | The device entrance | refused with a stable code; the manifest offers it to nobody |
//! | The app key | a marker in the address and in both exchanges, never a value |

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    OAuthFlowManifest, PluginManifest,
    extension::{AuthorizationRequest, FolderCrawler, OAuthProvider, TokenOutcome},
};

const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/dropbox-crawler/manifest.toml");
const OAUTH_MANIFEST: &str = include_str!("../../../plugins/dropbox-oauth/manifest.toml");
const RESOLVER_MANIFEST: &str = include_str!("../../../plugins/dropbox/manifest.toml");

// The sanitised fixtures. Every token-shaped value in them is a placeholder and nothing else;
// `fixtures_carry_no_credential_material` is what keeps it that way.
const TOKEN_SUCCESS: &str = include_str!("fixtures/dropbox/token_success.json");
const TOKEN_RENEWED: &str = include_str!("fixtures/dropbox/token_renewed.json");
const TOKEN_DENIED: &str = include_str!("fixtures/dropbox/token_denied.json");
const TOKEN_INVALID_GRANT: &str = include_str!("fixtures/dropbox/token_invalid_grant.json");
const TOKEN_RATE_LIMITED: &str = include_str!("fixtures/dropbox/token_rate_limited.json");
const FOLDER_PAGE_1: &str = include_str!("fixtures/dropbox/folder_page_1.json");
const FOLDER_PAGE_2: &str = include_str!("fixtures/dropbox/folder_page_2.json");
const FOLDER_PAGE_3: &str = include_str!("fixtures/dropbox/folder_page_3.json");
const ERROR_NOT_FOUND: &str = include_str!("fixtures/dropbox/error_path_not_found.json");
const ERROR_ACCESS_DENIED: &str =
    include_str!("fixtures/dropbox/error_shared_link_access_denied.json");
const ERROR_EXPIRED_TOKEN: &str = include_str!("fixtures/dropbox/error_expired_access_token.json");
const ERROR_TOO_MANY: &str = include_str!("fixtures/dropbox/error_too_many_requests.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";

/// The shared folder link every shared-link case uses, as the API takes it.
const SHARED_LINK: &str =
    "https://www.dropbox.com/scl/fo/redacted-link-id/redacted-hash?rlkey=redacted-rlkey";

/// A bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn crawler_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-dropbox-crawler")
}

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-dropbox-oauth")
}

// -- the mock Dropbox API -------------------------------------------------------------------

/// One folder the mock holds: its API path (absolute in the account, relative to the link's
/// root inside a shared link), its name, and the `list_folder` pages it answers with, in order.
struct Folder {
    path: &'static str,
    name: &'static str,
    pages: Vec<String>,
}

/// The mock Dropbox API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:dropbox_access_token}}` and never as a value.
/// Pages are followed the way Dropbox follows them: `list_folder/continue` is answered with the
/// page after the one whose `cursor` was quoted, so a walk that forgot the cursor repeats page
/// one and a walk that invented one gets nothing.
struct MockDropbox {
    folders: Vec<Folder>,
    /// The password the shared link wants, when it wants one.
    link_password: Option<&'static str>,
    requests: Mutex<Vec<String>>,
    bodies: Mutex<Vec<serde_json::Value>>,
    authorizations: Mutex<Vec<String>>,
    /// When set, every request fails with this status and this body instead of being answered.
    failure: Option<(u16, &'static str)>,
}

impl MockDropbox {
    fn new(folders: Vec<Folder>) -> Arc<Self> {
        Arc::new(Self {
            folders,
            link_password: None,
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    fn password_protected(folders: Vec<Folder>, password: &'static str) -> Arc<Self> {
        Arc::new(Self {
            folders,
            link_password: Some(password),
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    fn failing(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            folders: Vec::new(),
            link_password: None,
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: Some((status, body)),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    fn bodies(&self) -> Vec<serde_json::Value> {
        self.bodies.lock().expect("bodies").clone()
    }

    fn folder(&self, path: &str) -> Option<&Folder> {
        self.folders
            .iter()
            .find(|folder| folder.path.eq_ignore_ascii_case(path))
    }

    /// The page after the one that carried `cursor`, wherever it was.
    fn page_after(&self, cursor: &str) -> Option<&String> {
        let quoted = format!("\"cursor\": \"{cursor}\"");
        let compact = format!("\"cursor\":\"{cursor}\"");
        self.folders.iter().find_map(|folder| {
            let index = folder
                .pages
                .iter()
                .position(|page| page.contains(&quoted) || page.contains(&compact))?;
            folder.pages.get(index + 1)
        })
    }
}

/// One `list_folder` page, generated: `entries`, a cursor naming the folder and the page, and
/// whether there is more.
fn page(folder: &str, index: usize, entries: &[String], more: bool) -> String {
    format!(
        r#"{{"entries":[{}],"cursor":"cursor:{folder}:{index}","has_more":{more}}}"#,
        entries.join(",")
    )
}

fn file(name: &str, size: u64) -> String {
    format!(
        r#"{{".tag":"file","name":"{name}","id":"id:{name}","rev":"015f3d2a1b2c3d4e5f6a7","size":{size},"is_downloadable":true}}"#
    )
}

fn paper(name: &str) -> String {
    format!(r#"{{".tag":"file","name":"{name}","id":"id:{name}","is_downloadable":false}}"#)
}

fn subfolder(name: &str) -> String {
    format!(r#"{{".tag":"folder","name":"{name}","id":"id:{name}"}}"#)
}

#[async_trait]
impl ResolverHost for MockDropbox {
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
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
        self.bodies.lock().expect("bodies").push(body.clone());
        let text = |name: &str| body.get(name).and_then(serde_json::Value::as_str);
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
        let endpoint = request.url.path().to_owned();
        if let Some((status, body)) = self.failure {
            self.requests
                .lock()
                .expect("requests")
                .push(format!("failed {endpoint}"));
            return answer(status, body.to_owned());
        }
        // A password-protected link refuses every call that does not quote its password —
        // in `link_password` on the sharing endpoint, in `shared_link.password` on the listing.
        if let Some(expected) = self.link_password {
            let given = text("link_password").or_else(|| {
                body.get("shared_link")
                    .and_then(|link| link.get("password"))
                    .and_then(serde_json::Value::as_str)
            });
            let shared = text("url").is_some() || body.get("shared_link").is_some();
            if shared && given != Some(expected) {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("denied {endpoint}"));
                return answer(409, ERROR_ACCESS_DENIED.to_owned());
            }
        }
        match endpoint.as_str() {
            "/2/files/get_metadata" | "/2/sharing/get_shared_link_metadata" => {
                let path = text("path").unwrap_or_default().to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("metadata {path}"));
                match self.folder(&path) {
                    Some(folder) => answer(
                        200,
                        format!(
                            r#"{{".tag":"folder","name":"{}","id":"id:folder"}}"#,
                            folder.name
                        ),
                    ),
                    None => answer(409, ERROR_NOT_FOUND.to_owned()),
                }
            }
            "/2/files/list_folder" => {
                let path = text("path").unwrap_or_default().to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("list {path}"));
                match self.folder(&path).and_then(|folder| folder.pages.first()) {
                    Some(first) => answer(200, first.clone()),
                    None => answer(409, ERROR_NOT_FOUND.to_owned()),
                }
            }
            "/2/files/list_folder/continue" => {
                let cursor = text("cursor").unwrap_or_default().to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("continue {cursor}"));
                match self.page_after(&cursor) {
                    Some(next) => answer(200, next.clone()),
                    None => answer(
                        409,
                        r#"{"error_summary":"reset/..","error":{".tag":"reset"}}"#.to_owned(),
                    ),
                }
            }
            _ => answer(404, String::new()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn crawler(host: Arc<MockDropbox>, bytes: &[u8]) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the crawler manifest");
    FolderCrawler::new(manifest, bytes, Some(host)).expect("compile the crawler")
}

// -- the crawler ----------------------------------------------------------------------------

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// The check that keeps a crawler from costing every other link in the queue a request — and,
/// here, the one that keeps the two siblings apart: a file address is the resolver's.
#[tokio::test]
async fn only_dropbox_folder_addresses_are_claimed() {
    let bytes = crawler_component();
    let host = MockDropbox::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&dl=0",
        "https://www.dropbox.com/sh/abc/h1/Season%201?dl=0",
        "https://www.dropbox.com/home/Show",
        "https://www.dropbox.com/home",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // The sibling resolver's addresses, every spelling of them.
        "https://www.dropbox.com/s/abc/release.bin?dl=0",
        "https://www.dropbox.com/scl/fi/abc/release.bin?rlkey=k1",
        "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&preview=e01.mkv",
        "https://www.dropbox.com/home/Show?preview=e01.mkv",
        // And somebody else's entirely.
        "https://dropbox.com.evil.test/sh/abc/h1",
        "https://ddownload.com/f/abc",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.requests().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A folder comes back as its files, with the names, the sizes and the structure Dropbox gave —
/// and each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![
        Folder {
            path: "/Show",
            name: "Show",
            pages: vec![page(
                "show",
                0,
                &[
                    file("readme.txt", 12),
                    subfolder("Season 1"),
                    paper("Notes"),
                ],
                false,
            )],
        },
        Folder {
            path: "/Show/Season 1",
            name: "Season 1",
            pages: vec![page(
                "season",
                0,
                &[file("e01.mkv", 1024), file("e02.mkv", 2048)],
                false,
            )],
        },
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.dropbox.com/home/Show", None)
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
    assert_eq!(
        links[1].url,
        "https://www.dropbox.com/home/Show/Season%201?preview=e01.mkv"
    );
    assert_eq!(
        host.requests(),
        vec![
            "metadata /Show".to_owned(),
            "list /Show".to_owned(),
            "list /Show/Season 1".to_owned(),
        ]
    );
}

/// Dropbox answers a wide folder a page at a time. Every cursor is followed, out of the walk's
/// own state, and the walk is not over until Dropbox says `has_more` is false.
///
/// The pages are the sanitised fixtures, cursors included: a walk that forgot the cursor would
/// be answered page one again, and one that invented a cursor would be answered `reset`.
#[tokio::test]
async fn a_folder_dropbox_answers_a_page_at_a_time_is_followed_by_its_cursor() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![Folder {
        path: "/Long",
        name: "Long",
        pages: vec![
            FOLDER_PAGE_1.to_owned(),
            FOLDER_PAGE_2.to_owned(),
            FOLDER_PAGE_3.to_owned(),
        ],
    }]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.dropbox.com/home/Long", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(links[2].file_name.as_deref(), Some("c.mkv"));
    assert_eq!(
        host.requests(),
        vec![
            "metadata /Long".to_owned(),
            "list /Long".to_owned(),
            "continue AAE_redacted-cursor-0001".to_owned(),
            "continue AAE_redacted-cursor-0002".to_owned(),
        ],
        "each cursor has to be carried into the next request, or the walk repeats page one"
    );
    // And the deleted entry on page one was dropped, not handed back.
    assert!(
        links
            .iter()
            .all(|link| link.file_name.as_deref() != Some("old.mkv"))
    );
}

/// A password-protected shared folder link is listed through the official `shared_link`
/// argument with its password, and every file it yields carries the password on — so a
/// crawled file resolves the same way its folder was listed.
#[tokio::test]
async fn a_shared_folder_link_is_listed_with_its_password_and_its_files_carry_it() {
    let bytes = crawler_component();
    let host = MockDropbox::password_protected(
        vec![
            Folder {
                path: "",
                name: "Shared",
                pages: vec![page(
                    "root",
                    0,
                    &[file("a.bin", 1), subfolder("Sub")],
                    false,
                )],
            },
            Folder {
                path: "/Sub",
                name: "Sub",
                pages: vec![page("sub", 0, &[file("b.bin", 2)], false)],
            },
        ],
        "hunter2",
    );
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARED_LINK}&dl=0&link_password=hunter2"), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(links[0].package_hint.as_deref(), Some("Shared"));
    assert_eq!(links[1].package_hint.as_deref(), Some("Shared/Sub"));
    assert_eq!(
        links[1].url,
        "https://www.dropbox.com/scl/fo/redacted-link-id/redacted-hash/Sub?rlkey=redacted-rlkey&preview=b.bin&link_password=hunter2"
    );
    // The listing went through `shared_link`, with the path relative to the link's root.
    let bodies = host.bodies();
    assert_eq!(bodies[0]["url"], SHARED_LINK);
    assert_eq!(bodies[0]["link_password"], "hunter2");
    assert_eq!(bodies[1]["path"], "");
    assert_eq!(bodies[1]["shared_link"]["url"], SHARED_LINK);
    assert_eq!(bodies[1]["shared_link"]["password"], "hunter2");
    assert_eq!(bodies[2]["path"], "/Sub");
}

/// The same link without its password is one refusal with its own code — the one thing a
/// person can fix from the address — and nothing is guessed.
#[tokio::test]
async fn a_password_protected_link_without_its_password_is_refused_by_name() {
    let bytes = crawler_component();
    let host = MockDropbox::password_protected(
        vec![Folder {
            path: "",
            name: "Shared",
            pages: vec![page("root", 0, &[file("a.bin", 1)], false)],
        }],
        "hunter2",
    );
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARED_LINK}&dl=0"), None)
        .await
        .expect("call")
        .expect_err("a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("dropbox_crawler.link_access_denied")
    );
    assert_eq!(host.requests().len(), 1, "refused once, not retried");
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![Folder {
        path: "/Show",
        name: "Show",
        pages: vec![page("show", 0, &[file("a.mkv", 1)], false)],
    }]);
    crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.dropbox.com/home/Show", None)
        .await
        .expect("call")
        .expect("a listing");

    let authorizations = host.authorizations.lock().expect("authorizations").clone();
    assert!(!authorizations.is_empty());
    for value in authorizations {
        assert_eq!(
            value, "Bearer {{secret:dropbox_access_token}}",
            "the plugin must name the reference, never hold the token"
        );
    }
}

/// An empty folder says so, with a code the interface can translate.
///
/// Returning an empty list would create a package with nothing in it and nothing to explain
/// why — the defect ADR 0001 was written for. A folder holding only a Paper document is empty
/// in the same sense: nothing in it can be downloaded.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![Folder {
        path: "/Empty",
        name: "Empty",
        pages: vec![page("empty", 0, &[paper("Notes")], false)],
    }]);
    let refusal = crawler(host, &bytes)
        .crawl("https://www.dropbox.com/home/Empty", None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("dropbox_crawler.folder_empty")
    );
}

/// The three ways a crawl can fail reach the person as three different codes, and none of them
/// repeats a word Dropbox wrote.
#[tokio::test]
async fn a_refused_token_a_rate_limit_and_a_missing_folder_are_told_apart() {
    let bytes = crawler_component();
    for (status, body, expected) in [
        (401, ERROR_EXPIRED_TOKEN, "dropbox_crawler.sign_in_required"),
        (429, ERROR_TOO_MANY, "dropbox_crawler.rate_limited"),
        (409, ERROR_NOT_FOUND, "dropbox_crawler.folder_unreachable"),
    ] {
        let refusal = crawler(MockDropbox::failing(status, body), &bytes)
            .crawl("https://www.dropbox.com/home/Show", None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(expected), "{status}");
        assert!(
            !refusal.message.contains("A sentence Dropbox wrote"),
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
        .crawl("https://www.dropbox.com/home/Show", None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

// -- the sign-in ----------------------------------------------------------------------------

/// What Dropbox's token endpoint should answer next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    /// Checks the PKCE verifier and, when it matches, hands the tokens over.
    Granted,
    /// A renewal: no PKCE, and an answer that carries no refresh material — which is exactly
    /// what Dropbox sends, because it issues that once and expects it to keep working.
    RenewalGranted,
    Denied,
    InvalidGrant,
    RateLimited,
    /// Dropbox could not be reached at all — not an answer, a failed call.
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

/// The mock Dropbox authorization server.
struct MockAuthorizer {
    case: Mutex<Case>,
    challenge: Mutex<Option<String>>,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockAuthorizer {
    fn new(case: Case) -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(case),
            challenge: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
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
        match *self.case.lock().expect("case") {
            Case::Granted => {
                // The check a real authorization server makes: the verifier the client kept
                // back has to hash to the challenge it published. Without it the "PKCE" here
                // would be decoration.
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
                "dropbox could not be reached",
            )),
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
    use base64::Engine as _;
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

fn provider(host: Arc<MockAuthorizer>, bytes: &[u8]) -> OAuthProvider {
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

/// Authorization code with PKCE, from the address the person is sent to all the way to the
/// stored token — and with the one parameter without which Dropbox issues no refresh material.
#[tokio::test]
async fn the_sign_in_runs_through_to_a_stored_token_with_its_renewal_material() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    assert!(
        request
            .authorization_url
            .starts_with("https://www.dropbox.com/oauth2/authorize?"),
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
    // Least privilege: read what the account can see, and nothing else. The same token is
    // spent by two other plugins, so a wider scope would widen all three at once.
    assert_eq!(
        parameter(&request, "scope").as_deref(),
        Some("account_info.read files.metadata.read files.content.read sharing.read")
    );
    // Without this Dropbox issues a short-lived token and nothing to renew it from.
    assert_eq!(
        parameter(&request, "token_access_type").as_deref(),
        Some("offline")
    );

    // No app key anywhere in this plugin: the marker travels and the host substitutes what
    // *this installation* registered (RD-106-04, rule 8). A key compiled into the package would
    // sit in the git history, in every signed `.rdplug`, and would put every installation in
    // the world on one shared Dropbox rate limit.
    assert!(
        request
            .authorization_url
            .contains("client_id=%7B%7Bclient_id%7D%7D")
            || request
                .authorization_url
                .contains("client_id={{client_id}}"),
        "the authorization URL must carry the marker, not an app key: {}",
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
    assert_eq!(exchange.url, "https://api.dropboxapi.com/oauth2/token");
    assert_eq!(exchange.field("grant_type"), Some("authorization_code"));
    assert_eq!(exchange.field("code_verifier"), Some(verifier.as_str()));
    // The same in the exchange. The mock stands where the host's expansion would be, so what
    // it sees is what the plugin wrote — which is the marker and never a value.
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    // A public client: no app secret travels, because there is nowhere it could live.
    assert!(exchange.field("client_secret").is_none());

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(14_400),
        }]
    );
}

/// A stolen code without its verifier buys nothing, which is the whole reason PKCE exists.
#[tokio::test]
async fn a_code_without_its_verifier_is_refused() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(Case::Granted);
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
    let plugin = provider(MockAuthorizer::new(Case::Granted), &bytes);
    let account = AccountId::new();

    let first = plugin.begin(account, None).await.expect("begin");
    let second = plugin.begin(account, None).await.expect("begin");
    assert_ne!(first.state, second.state, "the state repeated");
    assert_ne!(first.flow_state, second.flow_state, "the verifier repeated");
    assert_ne!(
        first.state,
        first.flow_state.clone().expect("a verifier"),
        "the state and the verifier are the same value"
    );
    for request in [&first, &second] {
        assert!(!request.state.contains(&account.to_string()));
    }
}

/// A refusal ends the flow and an unreachable Dropbox does not — the distinction the renewal
/// sweep acts on, and the one that would otherwise sign people out whenever a connection drops.
#[tokio::test]
async fn a_refused_grant_ends_the_flow_and_an_unreachable_dropbox_does_not() {
    let bytes = oauth_component();
    let account = AccountId::new();

    for (case, expected) in [
        (Case::Denied, "access_denied"),
        (Case::InvalidGrant, "invalid_grant"),
    ] {
        let server = MockAuthorizer::new(case);
        let outcome = provider(server.clone(), &bytes)
            .refresh(account, Some("dropbox_access_token"))
            .await
            .expect("refresh");
        let TokenOutcome::Failed { message, .. } = outcome else {
            panic!("{case:?} should have failed, got {outcome:?}");
        };
        // Dropbox's own error code survives; the sentence beside it does not.
        assert!(message.contains(expected), "{message}");
        assert!(
            !message.contains("code has expired") && !message.contains("chose not to"),
            "dropbox's prose reached the message: {message}"
        );
        assert!(server.stored().is_empty());
    }

    let offline = MockAuthorizer::new(Case::Unreachable);
    let outcome = provider(offline.clone(), &bytes)
        .refresh(account, Some("dropbox_access_token"))
        .await;
    assert!(
        outcome.is_err(),
        "an unreachable provider must not end the flow"
    );
    assert!(offline.stored().is_empty());
}

/// A renewal stores the new token without anybody being asked, and the stored refresh material
/// leaves the plugin as a template rather than as a value.
///
/// This is what a Dropbox account lives on: its access tokens last four hours, so without it
/// every download started later than that after a sign-in would ask for a new one.
#[tokio::test]
async fn a_renewal_stores_a_new_token_without_a_redirect() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(Case::RenewalGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .refresh(account, Some("dropbox_access_token"))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some("{{secret:dropbox_access_token}}")
    );
    assert_eq!(sent.field("client_id"), Some("{{client_id}}"));
    assert!(sent.field("code").is_none(), "a renewal carries no code");
    // Dropbox hands refresh material over once and expects it to keep working, so an answer
    // without it is still a grant. Reading it as a failure would end every renewal but the
    // first.
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: None,
            expires_in_seconds: Some(14_400),
        }]
    );
}

/// A rate limit is a wait, not a failure, and carries Dropbox's own `Retry-After`.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(Case::RateLimited);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let renewed = plugin
        .refresh(account, Some("dropbox_access_token"))
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

/// The device entrance is not offered, and being asked anyway is a stable refusal rather than
/// a trap. Dropbox has no device flow at all.
#[tokio::test]
async fn the_device_entrance_is_refused_with_a_stable_code() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let failure = plugin
        .device_begin(account, None)
        .await
        .expect_err("no device entrance");
    assert!(
        format!("{failure}").contains("flow_unsupported")
            || format!("{failure:?}").contains("flow_unsupported"),
        "{failure:?}"
    );
    assert!(
        server.requests().is_empty(),
        "a flow that is not offered must ask dropbox nothing"
    );
}

// -- the shape of the three packages ---------------------------------------------------------

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dropbox");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        for field in ["access_token", "refresh_token", "id_token", "code"] {
            let Some(found) = value.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            assert!(
                found == PLACEHOLDER_ACCESS_TOKEN || found == PLACEHOLDER_REFRESH_TOKEN,
                "{path:?} carries a `{field}` that is not a placeholder"
            );
        }
        // A cursor is a stateful value Dropbox issues per account; the fixtures carry the
        // redacted shape only.
        if let Some(cursor) = value.get("cursor").and_then(serde_json::Value::as_str) {
            assert!(
                cursor.starts_with("AAE_redacted"),
                "{path:?} carries a real cursor"
            );
        }
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures were checked");
}

/// The three siblings reach only Dropbox, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying the
/// provider row, one crawler and one sign-in claiming that row's slug, and no capability beyond
/// HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_dropbox_each_one_needs() {
    let resolver: PluginManifest = toml::from_str(RESOLVER_MANIFEST).expect("resolver manifest");
    let crawler: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("crawler manifest");
    let sign_in: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("oauth manifest");

    // Only a resolver manifest may carry a provider row, so the account, its vault reference
    // and the slug the other two claim all live in exactly one of the three.
    let provider = resolver.provider.as_ref().expect("the provider row");
    assert_eq!(provider.slug, "dropbox");
    assert_eq!(
        provider.credentials,
        rd_plugin_host::CredentialKindManifest::OAuth
    );
    assert_eq!(
        provider.secret_reference.as_deref(),
        Some("dropbox_access_token")
    );
    // The account's username field holds this installation's own app key, and it is required.
    assert!(
        provider.username_required,
        "an OAuth provider whose client is registered per installation has to ask for it"
    );
    // The exact hosts the account's token may be sent to: the RPC host the resolver calls, and
    // the content host the transfer goes to — which is what lets the scheduler put the bearer
    // on the transfer itself (RD-106-04, rule 6).
    assert_eq!(
        provider.secret_domains,
        vec![
            "api.dropboxapi.com".to_owned(),
            "content.dropboxapi.com".to_owned()
        ]
    );
    assert_eq!(
        resolver.download_domains,
        vec!["content.dropboxapi.com".to_owned()]
    );
    assert!(crawler.provider.is_none() && sign_in.provider.is_none());

    for manifest in [&crawler, &sign_in] {
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec!["dropbox".to_owned()], "{}", manifest.name);
    }

    // The API for the two that talk to Dropbox; Dropbox's own sign-in hosts for the third.
    assert_eq!(
        resolver.capabilities.domains(),
        [
            "api.dropboxapi.com".to_owned(),
            "content.dropboxapi.com".to_owned()
        ]
        .as_slice()
    );
    assert_eq!(
        crawler.capabilities.domains(),
        ["api.dropboxapi.com".to_owned()].as_slice()
    );
    assert_eq!(
        sign_in.capabilities.domains(),
        [
            "www.dropbox.com".to_owned(),
            "api.dropboxapi.com".to_owned()
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

    // Redirect only, and stated rather than guessed at, so the host never calls the other.
    assert_eq!(
        sign_in.oauth_flows(),
        [OAuthFlowManifest::Redirect].as_slice()
    );
    assert!(!sign_in.serves_oauth_flow(OAuthFlowManifest::Device));
}

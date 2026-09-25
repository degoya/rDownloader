//! Google Drive, exercised end to end against a mock Drive API and a mock Google
//! authorization server (RD-106-04).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither googleapis.com nor accounts.google.com is contacted. A run against a real Google
//! account is *not* claimed here; there is none in this checkout, and there is no registered
//! OAuth client either. What is proven is everything that does not need one.
//!
//! Two of the three Google Drive plugins are driven here as real WebAssembly components,
//! because that is the only place their guest code exists: the crawler's paging walk and the
//! sign-in's exchange are written against the WIT imports and cannot run natively. The third,
//! `plugins/google-drive/`, keeps its protocol logic outside the component and is covered by
//! `plugins/google-drive/src/native/tests.rs` against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Drive answers a page at a time | every page followed, to the cap |
//! | A Workspace document inside a folder | the export name, extension already on it |
//! | A folder that contains itself | read once |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Quota, virus scan, rate limit, refused token | four different codes |
//! | The sign-in | PKCE through to a stored token and its renewal material |
//! | The device entrance | refused with a stable code; the manifest offers it to nobody |
//! | The OAuth client | a marker in the address and in both exchanges, never a value |

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

const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/google-drive-crawler/manifest.toml");
const OAUTH_MANIFEST: &str = include_str!("../../../plugins/google-drive-oauth/manifest.toml");
const RESOLVER_MANIFEST: &str = include_str!("../../../plugins/google-drive/manifest.toml");

// The sanitised fixtures. Every token-shaped value in them is a placeholder and nothing else;
// `fixtures_carry_no_credential_material` is what keeps it that way.
const TOKEN_SUCCESS: &str = include_str!("fixtures/google_drive/token_success.json");
const TOKEN_RENEWED: &str = include_str!("fixtures/google_drive/token_renewed.json");
const TOKEN_DENIED: &str = include_str!("fixtures/google_drive/token_denied.json");
const TOKEN_INVALID_GRANT: &str = include_str!("fixtures/google_drive/token_invalid_grant.json");
const TOKEN_RATE_LIMITED: &str = include_str!("fixtures/google_drive/token_rate_limited.json");
const FILE_QUOTA_EXCEEDED: &str = include_str!("fixtures/google_drive/file_quota_exceeded.json");
const FILE_VIRUS_SCAN: &str = include_str!("fixtures/google_drive/file_virus_scan_warning.json");
const FILE_RATE_LIMITED: &str = include_str!("fixtures/google_drive/file_rate_limited.json");
const FILE_AUTH_ERROR: &str = include_str!("fixtures/google_drive/file_auth_error.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";

const ROOT: &str = "redacted-folder-id-0001";

/// A bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn crawler_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-google-drive-crawler")
}

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-google-drive-oauth")
}

// -- the mock Drive API ---------------------------------------------------------------------

/// One `files.list` answer, as one page of one folder.
type Pages = Vec<String>;

/// The mock Drive API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:google_drive_access_token}}` and never as a value.
struct MockDrive {
    /// `(folder id, its display name, its pages)`.
    folders: Vec<(&'static str, &'static str, Pages)>,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    /// When set, every request fails with this status and this body instead of being answered.
    failure: Option<(u16, &'static str)>,
}

impl MockDrive {
    fn new(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            folders,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    fn failing(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
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

/// One `files.list` page.
fn page(entries: &[String], next: Option<&str>) -> String {
    let token = next.map_or(String::new(), |token| {
        format!(r#""nextPageToken":"{token}","#)
    });
    format!(r#"{{{token}"files":[{}]}}"#, entries.join(","))
}

fn file(id: &str, name: &str, size: u64) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","mimeType":"video/x-matroska","size":"{size}"}}"#)
}

fn document(id: &str, name: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","mimeType":"application/vnd.google-apps.spreadsheet"}}"#
    )
}

fn subfolder(id: &str, name: &str) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","mimeType":"application/vnd.google-apps.folder"}}"#)
}

#[async_trait]
impl ResolverHost for MockDrive {
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
        let value = |name: &str| {
            request
                .query
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.value_template.clone())
        };
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
        // `files.list` carries `q`; `files.get` does not. Told apart the way Drive tells them
        // apart, by the request rather than by the order the plugin happens to make them in.
        match value("q") {
            Some(query) => {
                let parent = query.split('\'').nth(1).unwrap_or_default().to_owned();
                let token = value("pageToken");
                self.requests.lock().expect("requests").push(format!(
                    "list {parent} {}",
                    token.clone().unwrap_or_default()
                ));
                let index = token
                    .and_then(|token| token.strip_prefix("page-").map(str::to_owned))
                    .and_then(|number| number.parse::<usize>().ok())
                    .unwrap_or(0);
                let pages = self
                    .folders
                    .iter()
                    .find(|(id, _, _)| *id == parent)
                    .map(|(_, _, pages)| pages);
                match pages.and_then(|pages| pages.get(index)) {
                    Some(body) => answer(200, body.clone()),
                    None => answer(200, page(&[], None)),
                }
            }
            None => {
                let id = request
                    .url
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("get {id}"));
                match self.folders.iter().find(|(known, _, _)| *known == id) {
                    Some((id, name, _)) => answer(
                        200,
                        format!(
                            r#"{{"id":"{id}","name":"{name}",
                                 "mimeType":"application/vnd.google-apps.folder"}}"#
                        ),
                    ),
                    None => answer(
                        404,
                        r#"{"error":{"code":404,"errors":[{"reason":"notFound"}]}}"#.to_owned(),
                    ),
                }
            }
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn crawler(host: Arc<MockDrive>, bytes: &[u8]) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the crawler manifest");
    FolderCrawler::new(manifest, bytes, Some(host)).expect("compile the crawler")
}

fn folder_address(id: &str) -> String {
    format!("https://drive.google.com/drive/folders/{id}")
}

// -- the crawler ----------------------------------------------------------------------------

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// The check that keeps a crawler from costing every other link in the queue a request — and,
/// here, the one that keeps the two siblings apart: a file address is the resolver's.
#[tokio::test]
async fn only_google_drive_folder_addresses_are_claimed() {
    let bytes = crawler_component();
    let host = MockDrive::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        "https://drive.google.com/drive/folders/1A2b3C",
        "https://drive.google.com/drive/u/0/folders/1A2b3C?usp=sharing",
        "https://drive.google.com/drive/u/1/shared-drives/1A2b3C",
        "https://drive.google.com/folderview?id=1A2b3C",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // The sibling resolver's addresses, every spelling of them.
        "https://drive.google.com/file/d/1A2b3C/view",
        "https://docs.google.com/document/d/1A2b3C/edit",
        "https://www.googleapis.com/drive/v3/files/1A2b3C?alt=media",
        // And somebody else's entirely.
        "https://drive.google.com.evil.test/drive/folders/1A2b3C",
        "https://ddownload.com/f/abc",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.requests().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A folder comes back as its files, with the names, the sizes and the structure Drive gave —
/// and each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![
        (
            ROOT,
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
        .crawl(&folder_address(ROOT), None)
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
    assert_eq!(links[1].url, "https://drive.google.com/file/d/x1/view");
    assert_eq!(
        host.requests(),
        vec![
            format!("get {ROOT}"),
            format!("list {ROOT} "),
            "list s1 ".to_owned(),
        ]
    );
}

/// Drive answers a wide folder a page at a time. Every page is followed, and the walk is not
/// over until Drive stops handing out a `nextPageToken`.
///
/// The limit the Premiumize crawler did not need: `folder/list` there answers whole.
#[tokio::test]
async fn a_folder_drive_answers_a_page_at_a_time_is_followed_to_the_end() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(
        ROOT,
        "Long",
        vec![
            page(&[file("x1", "a.mkv", 1)], Some("page-1")),
            page(&[file("x2", "b.mkv", 2)], Some("page-2")),
            page(&[file("x3", "c.mkv", 3)], None),
        ],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        host.requests(),
        vec![
            format!("get {ROOT}"),
            format!("list {ROOT} "),
            format!("list {ROOT} page-1"),
            format!("list {ROOT} page-2"),
        ],
        "the page token has to be carried into the next request, or the walk repeats page one"
    );
}

/// A Workspace document in a folder carries the name and the extension it will actually arrive
/// under, so it is visible in the LinkGrabber before anything is queued.
#[tokio::test]
async fn a_workspace_document_carries_its_export_name_before_anything_is_queued() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(
        ROOT,
        "Reports",
        vec![page(&[document("x1", "Quarterly numbers")], None)],
    )]);
    let links = crawler(host, &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].file_name.as_deref(),
        Some("Quarterly numbers.xlsx")
    );
    // No size: an export's bytes do not exist until the export runs, so stating one would be
    // inventing it.
    assert_eq!(links[0].size, None);
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(
        ROOT,
        "Show",
        vec![page(&[file("x1", "a.mkv", 1)], None)],
    )]);
    crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");

    let authorizations = host.authorizations.lock().expect("authorizations").clone();
    assert!(!authorizations.is_empty());
    for value in authorizations {
        assert_eq!(
            value, "Bearer {{secret:google_drive_access_token}}",
            "the plugin must name the reference, never hold the token"
        );
    }
}

/// A folder that contains itself is read once, not for ever.
///
/// Drive shortcuts make this reachable: a shortcut can point at an ancestor. The failure it
/// prevents cannot be spotted by watching — every request the walk makes is reasonable alone.
#[tokio::test]
async fn a_cycle_is_walked_once() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![
        (
            ROOT,
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
                    subfolder(ROOT, "Back"),
                    subfolder("inner", "Self"),
                    file("x2", "b.mkv", 2),
                ],
                None,
            )],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
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
///
/// Returning an empty list would create a package with nothing in it and nothing to explain
/// why — the defect ADR 0001 was written for.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(ROOT, "Empty", vec![page(&[], None)])]);
    let refusal = crawler(host, &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("google_drive_crawler.folder_empty")
    );
}

/// The three ways a crawl can fail reach the person as three different codes, and none of them
/// repeats a word Google wrote.
#[tokio::test]
async fn a_refused_token_a_rate_limit_and_a_missing_folder_are_told_apart() {
    let bytes = crawler_component();
    for (status, body, expected) in [
        (
            401,
            FILE_AUTH_ERROR,
            "google_drive_crawler.sign_in_required",
        ),
        (429, FILE_RATE_LIMITED, "google_drive_crawler.rate_limited"),
        // The two file-level 403s. A listing is not a download, so neither of them means here
        // what it means at the resolver — the folder simply could not be read, and the crawler
        // says exactly that rather than borrowing a warning about somebody's bytes.
        (
            403,
            FILE_QUOTA_EXCEEDED,
            "google_drive_crawler.folder_unreachable",
        ),
        (
            403,
            FILE_VIRUS_SCAN,
            "google_drive_crawler.folder_unreachable",
        ),
    ] {
        let refusal = crawler(MockDrive::failing(status, body), &bytes)
            .crawl(&folder_address(ROOT), None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(expected), "{status}");
        assert!(
            !refusal.message.contains("Invalid Credentials")
                && !refusal.message.contains("rate limit exceeded"),
            "a provider's prose reached the message: {}",
            refusal.message
        );
    }
}

/// A folder Drive says is gone is a refusal, not an empty folder.
#[tokio::test]
async fn a_folder_that_is_not_there_is_reported_with_its_own_code() {
    let bytes = crawler_component();
    let refusal = crawler(MockDrive::new(Vec::new()), &bytes)
        .crawl(&folder_address("redacted-folder-id-0009"), None)
        .await
        .expect("call")
        .expect_err("a missing folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("google_drive_crawler.folder_unreachable")
    );
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
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

// -- the sign-in ----------------------------------------------------------------------------

/// What Google's token endpoint should answer next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    /// Checks the PKCE verifier and, when it matches, hands the tokens over.
    Granted,
    /// A renewal: no PKCE, and an answer that carries no refresh material — which is exactly
    /// what Google sends, because it issues that once and expects it to keep working.
    RenewalGranted,
    Denied,
    InvalidGrant,
    RateLimited,
    /// Google could not be reached at all — not an answer, a failed call.
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

/// The mock Google authorization server.
struct MockGoogle {
    case: Mutex<Case>,
    challenge: Mutex<Option<String>>,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockGoogle {
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
impl ResolverHost for MockGoogle {
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
                "google could not be reached",
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

fn provider(host: Arc<MockGoogle>, bytes: &[u8]) -> OAuthProvider {
    let manifest: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("the oauth manifest");
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

/// Authorization code with PKCE, from the address the person is sent to all the way to the
/// stored token — and with the two parameters without which Google issues no refresh material
/// at all.
#[tokio::test]
async fn the_sign_in_runs_through_to_a_stored_token_with_its_renewal_material() {
    let bytes = oauth_component();
    let server = MockGoogle::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    assert!(
        request
            .authorization_url
            .starts_with("https://accounts.google.com/o/oauth2/v2/auth?"),
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
        Some("https://www.googleapis.com/auth/drive.readonly")
    );
    // Without both of these Google issues refresh material never, or exactly once — and an
    // account signed in a second time would come back with nothing to renew from.
    assert_eq!(
        parameter(&request, "access_type").as_deref(),
        Some("offline")
    );
    assert_eq!(parameter(&request, "prompt").as_deref(), Some("consent"));

    // No client id anywhere in this plugin: the marker travels and the host substitutes what
    // *this installation* registered (RD-106-04). A client id compiled into the package would
    // sit in the git history, in every signed `.rdplug`, and would put every installation in
    // the world on one shared Google quota.
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
    assert_eq!(exchange.url, "https://oauth2.googleapis.com/token");
    assert_eq!(exchange.field("grant_type"), Some("authorization_code"));
    assert_eq!(exchange.field("code_verifier"), Some(verifier.as_str()));
    // The same in the exchange. The mock stands where the host's expansion would be, so what
    // it sees is what the plugin wrote — which is the marker and never a value.
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
    let server = MockGoogle::new(Case::Granted);
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
    let plugin = provider(MockGoogle::new(Case::Granted), &bytes);
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

/// A refusal ends the flow and an unreachable Google does not — the distinction the renewal
/// sweep acts on, and the one that would otherwise sign people out whenever a connection drops.
#[tokio::test]
async fn a_refused_grant_ends_the_flow_and_an_unreachable_google_does_not() {
    let bytes = oauth_component();
    let account = AccountId::new();

    for (case, expected) in [
        (Case::Denied, "access_denied"),
        (Case::InvalidGrant, "invalid_grant"),
    ] {
        let server = MockGoogle::new(case);
        let outcome = provider(server.clone(), &bytes)
            .refresh(account, Some("google_drive_access_token"))
            .await
            .expect("refresh");
        let TokenOutcome::Failed { message, .. } = outcome else {
            panic!("{case:?} should have failed, got {outcome:?}");
        };
        // Google's own error code survives; the sentence beside it does not.
        assert!(message.contains(expected), "{message}");
        assert!(
            !message.contains("Token has been expired or revoked")
                && !message.contains("did not approve"),
            "google's prose reached the message: {message}"
        );
        assert!(server.stored().is_empty());
    }

    let offline = MockGoogle::new(Case::Unreachable);
    let outcome = provider(offline.clone(), &bytes)
        .refresh(account, Some("google_drive_access_token"))
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
/// This is what a Drive account lives on: Google's access tokens last an hour, so without it
/// every download started more than an hour after a sign-in would ask for a new one.
#[tokio::test]
async fn a_renewal_stores_a_new_token_without_a_redirect() {
    let bytes = oauth_component();
    let server = MockGoogle::new(Case::RenewalGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .refresh(account, Some("google_drive_access_token"))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some("{{secret:google_drive_access_token}}")
    );
    assert_eq!(sent.field("client_id"), Some("{{client_id}}"));
    assert!(sent.field("code").is_none(), "a renewal carries no code");
    // Google hands refresh material over once and expects it to keep working, so an answer
    // without it is still a grant. Reading it as a failure would end every renewal but the
    // first.
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: None,
            expires_in_seconds: Some(3599),
        }]
    );
}

/// A rate limit is a wait, not a failure, and carries Google's own `Retry-After`.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = oauth_component();
    let server = MockGoogle::new(Case::RateLimited);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let renewed = plugin
        .refresh(account, Some("google_drive_access_token"))
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
/// a trap. Google's device flow exists but does not grant the Drive scopes this needs.
#[tokio::test]
async fn the_device_entrance_is_refused_with_a_stable_code() {
    let bytes = oauth_component();
    let server = MockGoogle::new(Case::Granted);
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
        "a flow that is not offered must ask google nothing"
    );
}

// -- the shape of the three packages ---------------------------------------------------------

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/google_drive");
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
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures were checked");
}

/// The three siblings reach only Google, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests so RD-106-05 and RD-106-06
/// have something to copy that cannot silently drift: one resolver carrying the provider row,
/// one crawler and one sign-in claiming that row's slug, and no capability beyond HTTP to the
/// provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_google_each_one_needs() {
    let resolver: PluginManifest = toml::from_str(RESOLVER_MANIFEST).expect("resolver manifest");
    let crawler: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("crawler manifest");
    let sign_in: PluginManifest = toml::from_str(OAUTH_MANIFEST).expect("oauth manifest");

    // Only a resolver manifest may carry a provider row, so the account, its vault reference
    // and the slug the other two claim all live in exactly one of the three.
    let provider = resolver.provider.as_ref().expect("the provider row");
    assert_eq!(provider.slug, "google_drive");
    assert_eq!(
        provider.credentials,
        rd_plugin_host::CredentialKindManifest::OAuth
    );
    assert_eq!(
        provider.secret_reference.as_deref(),
        Some("google_drive_access_token")
    );
    // The account's username field holds this installation's own OAuth client id, and it is
    // required — a sign-in without one cannot start, and saying so at the form beats a Google
    // error page about a client that does not exist (RD-106-04).
    assert!(
        provider.username_required,
        "an OAuth provider whose client is registered per installation has to ask for it"
    );
    // The exact hosts the account's token may be sent to, and the ones the download engine
    // checks the transfer address against.
    assert_eq!(
        provider.secret_domains,
        vec!["www.googleapis.com".to_owned()]
    );
    assert!(crawler.provider.is_none() && sign_in.provider.is_none());

    for manifest in [&crawler, &sign_in] {
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec!["google_drive".to_owned()], "{}", manifest.name);
    }

    // The API for the two that talk to Drive; Google's own sign-in hosts for the third.
    assert_eq!(
        resolver.capabilities.domains(),
        ["www.googleapis.com".to_owned()].as_slice()
    );
    assert_eq!(
        crawler.capabilities.domains(),
        ["www.googleapis.com".to_owned()].as_slice()
    );
    assert_eq!(
        sign_in.capabilities.domains(),
        [
            "accounts.google.com".to_owned(),
            "oauth2.googleapis.com".to_owned()
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

//! The Nextcloud share crawler, exercised end to end against its built component.
//!
//! RD-107-05 wrote this plugin and tested its parser natively; what it could not show is
//! whether the *guest* behaves in the host the way the host expects. That is what this file
//! is for. Everything here runs the real `rd_plugin_nextcloud_crawler.wasm` inside Wasmtime,
//! under the manifest the plugin ships.
//!
//! **What that does and does not cover of RD-107-05.** Host gap 2 is in the loop: the manifest
//! says `*`, `controlled_http_request` measures every address against the list the host built
//! for this one call, and a request arriving at the mock therefore proves the narrowing admits
//! the pasted host. Its other half is not — the sandbox's own refusal is stood in for by a host
//! that fabricates `plugin.http_target_not_allowed`, so what is asserted there is that the
//! refusal is reported rather than turned into an empty share, not that the allow-list produces
//! it. Host gap 1 is not covered at all: `method_allowed` and `allowed_header` live in
//! `NativeHost::http_request`, which is exactly the production host these tests replace with a
//! mock. What is proven here is that the *guest* sends a `PROPFIND` with `Depth: 1` and
//! `X-Requested-With`; that the host lets those through is `rd-plugin-host`'s own tests.
//!
//! **The server is a mock.** It answers at the host boundary, so no socket is opened and no
//! Nextcloud is contacted. A run against a live instance is a separate acceptance and is
//! recorded in the job file, not claimed here.
//!
//! The documents are the fixtures under `tests/fixtures/nextcloud_crawler/`: a Nextcloud 29
//! `Depth: 1` multistatus on the public DAV endpoint, an ownCloud 10 answer on the older
//! endpoint with a different namespace prefix and no `displayname` on its files, and the
//! pages and sabre/dav error documents a share that is locked, gone or never existed
//! actually returns.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_host::{PluginManifest, extension::FolderCrawler};

const MANIFEST: &str = include_str!("../../../plugins/nextcloud-crawler/manifest.toml");

const SHARE_ROOT: &str = include_str!("fixtures/nextcloud_crawler/share_root.xml");
const SHARE_SEASON: &str = include_str!("fixtures/nextcloud_crawler/share_season.xml");
const SHARE_RAW: &str = include_str!("fixtures/nextcloud_crawler/share_raw.xml");
const SHARE_EMPTY: &str = include_str!("fixtures/nextcloud_crawler/share_empty.xml");
const SHARE_SINGLE_FILE: &str = include_str!("fixtures/nextcloud_crawler/share_single_file.xml");
const OWNCLOUD_ROOT: &str = include_str!("fixtures/nextcloud_crawler/owncloud_root.xml");
const UNAUTHORIZED: &str = include_str!("fixtures/nextcloud_crawler/unauthorized.xml");
const FORBIDDEN: &str = include_str!("fixtures/nextcloud_crawler/forbidden.xml");
const NOT_FOUND: &str = include_str!("fixtures/nextcloud_crawler/not_found.html");
const LOGIN_PAGE: &str = include_str!("fixtures/nextcloud_crawler/login_page.html");
const NESTED_LEVEL: &str = include_str!("fixtures/nextcloud_crawler/nested_level.xml");
const WIDE_LISTING: &str = include_str!("fixtures/nextcloud_crawler/wide_listing.xml");

/// The public DAV endpoint Nextcloud 29 and later serve for the share token the fixtures
/// were written around, without its trailing slash. Fifteen characters, as Nextcloud mints.
const DAV: &str = "/public.php/dav/files/QxT7bK2mNp9wZr4";
/// The endpoint older Nextcloud and every ownCloud serve.
const LEGACY: &str = "/public.php/webdav";

/// The address a person pastes.
const SHARE_URL: &str = "https://cloud.example.org/s/QxT7bK2mNp9wZr4";

/// `Authorization` for the share password `s3cret`, as the modern endpoint wants it: the
/// user name is fixed to `anonymous` and the password is the share's.
const BASIC_S3CRET: &str = "Basic YW5vbnltb3VzOnMzY3JldA==";

/// The bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-nextcloud-crawler")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the crawler manifest")
}

/// One request the plugin made, as the host saw it.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A Nextcloud, or something pretending to be one, answering at the host boundary.
struct MockNextcloud {
    /// Path to the status and body served for it, in order of preference.
    pages: Vec<(String, (u16, String))>,
    /// What every unlisted path answers; a 404 page by default.
    fallback: (u16, String),
    /// When not empty, only a request carrying one of these `Authorization` values is
    /// served; every other request is answered 401 with sabre/dav's own document. A list
    /// rather than one value because a protected share is probed on the modern endpoint
    /// before it falls back to the old one, and the two want different user names.
    passwords: Vec<String>,
    seen: Mutex<Vec<Seen>>,
}

impl MockNextcloud {
    fn new(pages: Vec<(&str, &str)>) -> Arc<Self> {
        Arc::new(Self {
            pages: pages
                .into_iter()
                .map(|(path, body)| (path.to_owned(), (207, body.to_owned())))
                .collect(),
            fallback: (404, NOT_FOUND.to_owned()),
            passwords: Vec::new(),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// A server that answers every address with one status and one document.
    fn answering(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            pages: Vec::new(),
            fallback: (status, body.to_owned()),
            passwords: Vec::new(),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// The whole share behind one password: anything without exactly this `Authorization`
    /// is answered the way sabre/dav answers it, with a 401 and its own document.
    fn locked(expected: &str, pages: Vec<(&str, &str)>) -> Arc<Self> {
        Arc::new(Self {
            pages: pages
                .into_iter()
                .map(|(path, body)| (path.to_owned(), (207, body.to_owned())))
                .collect(),
            fallback: (404, NOT_FOUND.to_owned()),
            passwords: vec![expected.to_owned()],
            seen: Mutex::new(Vec::new()),
        })
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .expect("seen")
            .iter()
            .map(|request| request.path.clone())
            .collect()
    }

    fn first(&self) -> Seen {
        self.seen
            .lock()
            .expect("seen")
            .first()
            .cloned()
            .expect("at least one request")
    }

    fn last(&self) -> Seen {
        self.seen
            .lock()
            .expect("seen")
            .last()
            .cloned()
            .expect("at least one request")
    }
}

#[async_trait]
impl ResolverHost for MockNextcloud {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let seen = Seen {
            method: request.method.clone(),
            path: request.url.path().to_owned(),
            headers: request
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value_template.clone()))
                .collect(),
        };
        let authorized = self.passwords.is_empty()
            || self
                .passwords
                .iter()
                .any(|expected| seen.header("authorization") == Some(expected.as_str()));
        self.seen.lock().expect("seen").push(seen);
        let (status, body) = if authorized {
            self.pages
                .iter()
                .find(|(path, _)| path == request.url.path())
                .map_or_else(|| self.fallback.clone(), |(_, answer)| answer.clone())
        } else {
            (401, UNAUTHORIZED.to_owned())
        };
        Ok(HostHttpResponse {
            status,
            final_url: request.url.clone(),
            headers: Vec::new(),
            body: body.into_bytes(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

fn crawler(host: Arc<MockNextcloud>, bytes: &[u8]) -> FolderCrawler {
    FolderCrawler::new(manifest(), bytes, Some(host)).expect("compile the crawler")
}

/// The whole share, as three fixtures on the modern endpoint.
fn whole_share() -> Vec<(&'static str, &'static str)> {
    vec![
        ("/public.php/dav/files/QxT7bK2mNp9wZr4/", SHARE_ROOT),
        (
            "/public.php/dav/files/QxT7bK2mNp9wZr4/Season%201/",
            SHARE_SEASON,
        ),
        (
            "/public.php/dav/files/QxT7bK2mNp9wZr4/Season%201/Raw/",
            SHARE_RAW,
        ),
    ]
}

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// A crawler that had to fetch to decide would cost every link in the queue a request, and
/// this one is asked about every address a person pastes.
#[tokio::test]
async fn only_share_shaped_addresses_are_claimed_and_deciding_reaches_nothing() {
    let bytes = component();
    let host = MockNextcloud::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        SHARE_URL,
        "https://cloud.example.org/index.php/s/QxT7bK2mNp9wZr4",
        "http://nc.example.net/cloud/s/QxT7bK2mNp9wZr4/",
        "https://cloud.example.org/s/QxT7bK2mNp9wZr4#s3cret",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // Too short to be a Nextcloud token: somebody else's two-letter path segment.
        "https://example.org/s/abc",
        // A path below the share is a file, and somebody else resolves it.
        "https://cloud.example.org/s/QxT7bK2mNp9wZr4/download/disc.iso",
        "https://cloud.example.org/apps/files/?dir=/Holiday",
        "ftp://cloud.example.org/s/QxT7bK2mNp9wZr4",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.paths().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// A public share becomes its files, with the names, the sizes and the folder structure the
/// server gave — and the shared folder's own name becomes the package suggestion.
///
/// This is the acceptance a person actually experiences, and it is also the one that proves
/// the two host gaps RD-107-05 closed are shut: the `PROPFIND` left the sandbox at all, and
/// a manifest that names no domain still reached the host of the pasted address.
#[tokio::test]
async fn a_public_share_becomes_its_files_with_names_sizes_and_structure() {
    let bytes = component();
    let host = MockNextcloud::new(whole_share());
    let crawler = crawler(Arc::clone(&host), &bytes);

    let links = crawler
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 5, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("beach sunset.jpg"));
    assert_eq!(links[0].size, Some(2_400_112));
    assert_eq!(links[0].package_hint.as_deref(), Some("Holiday 2024"));
    assert_eq!(
        links[0].url,
        format!("https://cloud.example.org{DAV}/beach%20sunset.jpg"),
        "the address stays percent-encoded: it is what the transfer will request"
    );
    assert_eq!(links[1].file_name.as_deref(), Some("packing list.txt"));
    assert_eq!(links[1].size, Some(4096));
    assert_eq!(links[2].file_name.as_deref(), Some("e01.mkv"));
    assert_eq!(
        links[2].package_hint.as_deref(),
        Some("Holiday 2024/Season 1"),
        "a subfolder extends the package name rather than replacing it"
    );
    assert_eq!(links[3].file_name.as_deref(), Some("e02.mkv"));
    assert_eq!(links[3].size, Some(2_097_152));
    // The entity in the name is undone, and the name is not the address.
    assert_eq!(links[4].file_name.as_deref(), Some("clip & take.mov"));
    assert_eq!(links[4].size, Some(734_003_200));
    assert_eq!(
        links[4].package_hint.as_deref(),
        Some("Holiday 2024/Season 1/Raw")
    );

    // Breadth first, each folder read exactly once, and the modern endpoint throughout.
    assert_eq!(
        host.paths(),
        vec![
            format!("{DAV}/"),
            format!("{DAV}/Season%201/"),
            format!("{DAV}/Season%201/Raw/"),
        ]
    );
}

/// The listing is a `PROPFIND` carrying the three headers Nextcloud's public endpoint needs.
///
/// All three used to be impossible from a crawler: the method was behind the write gate and
/// `Depth` was not an allowed header at all (RD-107-05, host gap 1). This asserts the guest's
/// half of that — the request is shaped the way the endpoint needs — and nothing about the
/// host's allow-list, which lives in `NativeHost::http_request` and is replaced here by the
/// mock. `native::expand::method_allowed` and `allowed_header` are tested where they live.
#[tokio::test]
async fn the_listing_is_a_propfind_with_the_headers_the_public_endpoint_needs() {
    let bytes = component();
    let host = MockNextcloud::new(whole_share());
    crawler(Arc::clone(&host), &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");

    let request = host.first();
    assert_eq!(request.method, "PROPFIND");
    assert_eq!(request.header("Depth"), Some("1"));
    assert_eq!(request.header("X-Requested-With"), Some("XMLHttpRequest"));
    assert_eq!(
        request.header("Content-Type"),
        Some("application/xml; charset=utf-8")
    );
    assert_eq!(
        request.header("Authorization"),
        None,
        "a public share sends no credential at all"
    );
}

/// An ownCloud, or a Nextcloud older than 29, is read through the older endpoint — and the
/// modern one is tried first, so a current instance never pays for the fallback.
///
/// The fixture is an ownCloud 10 answer: another namespace prefix, and files the server
/// names only by their address.
#[tokio::test]
async fn an_older_instance_is_read_through_the_legacy_endpoint() {
    let bytes = component();
    let host = MockNextcloud::new(vec![("/public.php/webdav/", OWNCLOUD_ROOT)]);
    let crawler = crawler(Arc::clone(&host), &bytes);

    let links = crawler
        .crawl("https://oc.example.org/index.php/s/mK4pQ8vR2tY6nL0", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(
        links[0].file_name.as_deref(),
        Some("minutes and notes.txt"),
        "no displayname: the name comes from the address, decoded"
    );
    assert_eq!(links[0].size, Some(8123));
    assert_eq!(links[0].package_hint.as_deref(), Some("Project files"));
    assert_eq!(
        links[0].url,
        "https://oc.example.org/public.php/webdav/minutes%20and%20notes.txt"
    );
    assert_eq!(links[1].file_name.as_deref(), Some("contract.pdf"));
    assert_eq!(links[1].size, Some(254_113));

    assert_eq!(
        host.paths(),
        vec![
            "/public.php/dav/files/mK4pQ8vR2tY6nL0/".to_owned(),
            format!("{LEGACY}/"),
        ],
        "the modern endpoint is asked first and its 404 is the signal to fall back"
    );
}

/// A share of a single file is that file, not a package with one thing in it and not the
/// refusal the walk would produce for a listing with no folder in it.
#[tokio::test]
async fn a_share_of_one_file_is_handed_back_as_that_file() {
    let bytes = component();
    let host = MockNextcloud::new(vec![(&format!("{DAV}/"), SHARE_SINGLE_FILE)]);
    let links = crawler(host, &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("annual report.pdf"));
    assert_eq!(links[0].size, Some(1_835_008));
    assert_eq!(links[0].package_hint, None);
}

/// A share nested deeper than the plugin walks stops at its own limit rather than following
/// a stranger's tree to the bottom.
///
/// The limit belongs to the plugin (`walk::MAX_DEPTH`), and the point of asserting it here
/// rather than in the plugin's own tests is that the *component* obeys it: a guest that
/// ignored its own bookkeeping would make a request per level for ever.
#[tokio::test]
async fn a_tree_deeper_than_the_limit_stops_at_the_limit() {
    let bytes = component();
    // Eight levels of the same document, each naming the one below it. The walk is allowed
    // four levels under the share, so the deepest level it may read is `deeper/` four times.
    let mut pages: Vec<(String, (u16, String))> = Vec::new();
    for level in 0..8u32 {
        let own = format!("{DAV}/{}", "deeper/".repeat(level as usize));
        let child = format!("{own}deeper/");
        pages.push((
            own.clone(),
            (
                207,
                NESTED_LEVEL
                    .replace("__SELF__", &own)
                    .replace("__CHILD__", &child)
                    .replace("__LEVEL__", &level.to_string()),
            ),
        ));
    }
    let host = Arc::new(MockNextcloud {
        pages,
        fallback: (404, NOT_FOUND.to_owned()),
        passwords: Vec::new(),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");

    // The share itself plus four levels below it: five folders read, five files found.
    assert_eq!(host.paths().len(), 5, "{:?}", host.paths());
    assert_eq!(links.len(), 5, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("note0.txt"));
    assert_eq!(links[4].file_name.as_deref(), Some("note4.txt"));
    assert_eq!(
        links[4].package_hint.as_deref(),
        Some("level 0/deeper/deeper/deeper/deeper")
    );
}

/// A subfolder that cannot be read is a hole in the tree, not the end of the crawl.
///
/// Losing one folder beats losing the other four hundred files, and the decision is the
/// guest's: the host would happily return the failure.
#[tokio::test]
async fn a_subfolder_that_is_refused_costs_only_that_subfolder() {
    let bytes = component();
    let host = Arc::new(MockNextcloud {
        pages: vec![
            (format!("{DAV}/"), (207, SHARE_ROOT.to_owned())),
            (format!("{DAV}/Season%201/"), (403, FORBIDDEN.to_owned())),
        ],
        fallback: (404, NOT_FOUND.to_owned()),
        passwords: Vec::new(),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(
        links.len(),
        2,
        "the two files of the share itself: {links:?}"
    );
    assert_eq!(links[0].file_name.as_deref(), Some("beach sunset.jpg"));
    assert_eq!(host.paths().len(), 2, "the refused folder was still tried");
}

/// The three addresses the job names — empty, locked, missing — each end with their own
/// stable code rather than with an empty list or a line nobody can act on.
#[tokio::test]
async fn an_empty_a_locked_and_a_missing_share_each_end_with_their_own_code() {
    let bytes = component();

    // Empty: the share exists, the server answered, and there is nothing in it.
    let empty = MockNextcloud::new(vec![(&format!("{DAV}/"), SHARE_EMPTY)]);
    let refusal = crawler(empty, &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect_err("an empty share is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("nextcloud_crawler.share_empty")
    );
    assert!(!refusal.not_mine, "the share is this plugin's; it is empty");

    // Locked, and no password was given with the address.
    let locked = MockNextcloud::answering(401, UNAUTHORIZED);
    let refusal = crawler(locked, &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect_err("a locked share is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("nextcloud_crawler.password_required")
    );
    assert!(!refusal.not_mine);

    // Missing: both endpoints say there is no such thing, so the address goes on to the
    // next crawler instead of ending here. This is RD-107-05's third host gap, end to end.
    let missing = MockNextcloud::new(Vec::new());
    let refusal = crawler(Arc::clone(&missing), &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect_err("a missing share is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("nextcloud_crawler.not_a_nextcloud")
    );
    assert!(
        refusal.not_mine,
        "a wrong guess hands the address on rather than ending the link"
    );
    assert_eq!(
        missing.paths(),
        vec![format!("{DAV}/"), format!("{LEGACY}/")],
        "both endpoints are tried before the plugin gives the address up"
    );
}

/// A withdrawn share, a busy server and a page that is not a listing are three different
/// answers, and each says which it is.
#[tokio::test]
async fn a_withdrawn_share_a_busy_server_and_a_login_page_are_told_apart() {
    let bytes = component();

    for (status, body, code) in [
        (403, FORBIDDEN, "nextcloud_crawler.share_unreachable"),
        (410, FORBIDDEN, "nextcloud_crawler.share_unreachable"),
        (429, "", "nextcloud_crawler.server_busy"),
        (503, "", "nextcloud_crawler.share_unreachable"),
        // A 200 that is a login page rather than a multistatus document.
        (200, LOGIN_PAGE, "nextcloud_crawler.invalid_response"),
    ] {
        let host = MockNextcloud::answering(status, body);
        let refusal = crawler(host, &bytes)
            .crawl(SHARE_URL, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(code), "status {status}");
        assert!(!refusal.not_mine, "status {status}");
    }
}

/// A password appended to the address opens the share, and a wrong one says so.
///
/// The password rides in the fragment, which never reaches a server, and leaves as
/// `Authorization: Basic` with the fixed user name the public endpoint wants.
#[tokio::test]
async fn a_share_password_opens_the_share_and_a_wrong_one_says_so() {
    let bytes = component();

    let host = MockNextcloud::locked("Basic YW5vbnltb3VzOnMzY3JldA==", whole_share());
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARE_URL}#s3cret"), None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 5, "{links:?}");
    assert_eq!(
        host.first().header("Authorization"),
        Some(BASIC_S3CRET),
        "the user name is fixed to anonymous on the modern endpoint"
    );

    let host = MockNextcloud::locked("Basic YW5vbnltb3VzOm90aGVy", whole_share());
    let refusal = crawler(host, &bytes)
        .crawl(&format!("{SHARE_URL}#s3cret"), None)
        .await
        .expect("call")
        .expect_err("a wrong password is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("nextcloud_crawler.password_wrong"),
        "a wrong password is not the same answer as no password"
    );
}

/// The files of a protected share come back with the login they need and without the
/// password, and the host turns that into one scoped credential (RD-108-07).
///
/// The defect this closes: the addresses a protected share was listed at carried nothing, so
/// the queue fetched them unauthenticated and every one of them failed. What travels now is
/// the user name the public endpoint fixes -- never the password, which the host already has
/// from the fragment and puts straight into the vault.
#[tokio::test]
async fn a_protected_share_hands_back_addresses_that_can_be_fetched() {
    let bytes = component();

    let host = MockNextcloud::locked(BASIC_S3CRET, whole_share());
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARE_URL}#s3cret"), None)
        .await
        .expect("call")
        .expect("a listing");

    let adopted: Vec<rd_plugin_ext::CrawledLink> = links
        .iter()
        .map(|link| {
            assert!(
                link.url
                    .starts_with(&format!("https://anonymous@cloud.example.org{DAV}/")),
                "every file carries the login the endpoint wants: {}",
                link.url
            );
            let (url, login) = rd_plugin_ext::split_crawled_address(&link.url)
                .expect("an address the host accepts");
            rd_plugin_ext::CrawledLink {
                url,
                file_name: link.file_name.clone(),
                size: link.size,
                package_hint: link.package_hint.clone(),
                mirror: None,
                login,
            }
        })
        .collect();

    // Nothing the plugin said carries the password -- not the address, not the name, not the
    // folder it was found in.
    for link in &links {
        let whole = format!(
            "{} {} {}",
            link.url,
            link.file_name.as_deref().unwrap_or_default(),
            link.package_hint.as_deref().unwrap_or_default()
        );
        assert!(!whole.contains("s3cret"), "{whole}");
        assert!(!whole.contains("czNjcmV0"), "not base64 either: {whole}");
    }
    for link in &adopted {
        assert!(link.url.password().is_none());
        assert!(link.url.username().is_empty(), "the stored address is bare");
    }

    let login = rd_plugin_ext::share_login(&adopted).expect("one login for the whole share");
    assert_eq!(login.username, "anonymous");
    assert_eq!(login.scope.host, "cloud.example.org");
    assert_eq!(
        login.scope.path_prefix.as_deref(),
        Some(DAV),
        "the credential reaches this share's files and stops there"
    );
    // And it matches what the queue will be asked about, which is what makes it work at all.
    assert!(login.scope.matches_url(&adopted[0].url));
    assert!(
        !login.scope.matches_url(
            &url::Url::parse("https://cloud.example.org/public.php/dav/files/Other/x")
                .expect("another share")
        ),
        "another share on the same server is not covered"
    );
}

/// The legacy endpoint names the share token as the user, and that is what comes back.
#[tokio::test]
async fn an_older_protected_instance_hands_back_its_token_as_the_login() {
    let bytes = component();

    // Both endpoints are protected; the modern one simply does not exist here, so the
    // fall-back happens on a 404 and the old endpoint is the one that answers.
    let legacy_basic = "Basic UXhUN2JLMm1OcDl3WnI0OnMzY3JldA==";
    let host = Arc::new(MockNextcloud {
        pages: vec![(format!("{LEGACY}/"), (207, OWNCLOUD_ROOT.to_owned()))],
        fallback: (404, NOT_FOUND.to_owned()),
        passwords: vec![BASIC_S3CRET.to_owned(), legacy_basic.to_owned()],
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARE_URL}#s3cret"), None)
        .await
        .expect("call")
        .expect("a listing");
    assert!(!links.is_empty());
    assert_eq!(
        host.last().header("Authorization"),
        Some(legacy_basic),
        "the old endpoint wants the share token as the user name"
    );
    for link in &links {
        assert!(
            link.url
                .starts_with("https://QxT7bK2mNp9wZr4@cloud.example.org/public.php/webdav"),
            "{}",
            link.url
        );
        assert!(!link.url.contains("s3cret"), "{}", link.url);
    }
}

/// A crawler reaches nothing the host did not grant it for this one call.
///
/// The manifest says `*`; what applies is the host of the pasted address, and a request
/// anywhere else is refused before a socket exists. The refusing host here is a stub that
/// answers with the code the sandbox would answer with — so what this asserts is the guest's
/// half, that the refusal is carried out rather than turned into an empty share. That the
/// allow-list produces the code is `rd-plugin-host`'s own test.
#[tokio::test]
async fn a_request_the_sandbox_refuses_is_reported_and_not_turned_into_an_empty_share() {
    let bytes = component();
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
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect_err("a refused request is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
}

/// An address this plugin never claimed is refused as not its own, without a request.
#[tokio::test]
async fn an_address_that_is_not_a_share_is_handed_on_without_a_request() {
    let bytes = component();
    let host = MockNextcloud::new(whole_share());
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl("https://cloud.example.org/apps/files/", None)
        .await
        .expect("call")
        .expect_err("not a share");
    assert_eq!(
        refusal.code.as_deref(),
        Some("nextcloud_crawler.not_a_share")
    );
    assert!(refusal.not_mine);
    assert!(host.paths().is_empty(), "nothing was fetched");
}

/// How many folders one crawl of this plugin reads, and how many files it hands back.
///
/// The numbers belong to `plugins/nextcloud-crawler/src/walk.rs` (`MAX_FOLDERS`, `MAX_FILES`)
/// and are repeated here rather than imported: a contract test asserts what the component
/// does, and a constant shared with the code under test would move with it.
const MAX_FOLDERS: usize = 100;
const MAX_FILES: usize = 500;

/// One `<d:response>` for a subfolder, in the shape every fixture in this directory carries.
fn wide_folder(root: &str, index: usize) -> String {
    format!(
        "  <d:response>\n    <d:href>{root}n{index}/</d:href>\n    <d:propstat>\n      \
         <d:prop>\n        <d:displayname>n{index}</d:displayname>\n        \
         <d:resourcetype>\n          <d:collection/>\n        </d:resourcetype>\n      \
         </d:prop>\n      <d:status>HTTP/1.1 200 OK</d:status>\n    </d:propstat>\n  \
         </d:response>"
    )
}

/// One `<d:response>` for a file, in the same shape.
fn wide_file(root: &str, index: usize) -> String {
    format!(
        "  <d:response>\n    <d:href>{root}f{index}.bin</d:href>\n    <d:propstat>\n      \
         <d:prop>\n        <d:displayname>f{index}.bin</d:displayname>\n        \
         <d:getcontentlength>1024</d:getcontentlength>\n        <d:resourcetype/>\n      \
         </d:prop>\n      <d:status>HTTP/1.1 200 OK</d:status>\n    </d:propstat>\n  \
         </d:response>"
    )
}

/// The wide-listing envelope filled with `entries`.
fn wide_listing(root: &str, entries: &[String]) -> String {
    WIDE_LISTING
        .replace("__SELF__", root)
        .replace("__ENTRIES__", &entries.join("\n"))
}

/// A share wider than the plugin reads stops at its own breadth limits rather than following
/// a stranger's folder however far it goes sideways.
///
/// The depth test covers the tree going down; this one covers it going across, which is the
/// cheaper attack: one folder holding a hundred thousand names costs one document to write
/// and a hundred thousand requests to obey. Both limits are asserted at the component,
/// because a guest that ignored its own bookkeeping would make every one of those requests
/// and each one would look perfectly reasonable.
#[tokio::test]
async fn a_share_wider_than_the_limits_stops_at_them() {
    let bytes = component();

    // Half the folders anybody would want, one and a half times what is read.
    let root = format!("{DAV}/");
    let siblings: Vec<String> = (0..150).map(|index| wide_folder(&root, index)).collect();
    let mut pages: Vec<(String, (u16, String))> =
        vec![(root.clone(), (207, wide_listing(&root, &siblings)))];
    for index in 0..150usize {
        let own = format!("{root}n{index}/");
        pages.push((
            own.clone(),
            (
                207,
                NESTED_LEVEL
                    .replace("__SELF__", &own)
                    .replace("__CHILD__", &format!("{own}deeper/"))
                    .replace("__LEVEL__", &index.to_string()),
            ),
        ));
    }
    let host = Arc::new(MockNextcloud {
        pages,
        fallback: (404, NOT_FOUND.to_owned()),
        passwords: Vec::new(),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(
        host.seen.lock().expect("seen").len(),
        MAX_FOLDERS,
        "the shared folder and ninety-nine of its subfolders, and not one more"
    );
    assert_eq!(
        links.len(),
        MAX_FOLDERS - 1,
        "one file out of each subfolder that was read"
    );

    // And the file limit, which a single folder can reach on its own.
    let many: Vec<String> = (0..600).map(|index| wide_file(&root, index)).collect();
    let host = Arc::new(MockNextcloud {
        pages: vec![(root.clone(), (207, wide_listing(&root, &many)))],
        fallback: (404, NOT_FOUND.to_owned()),
        passwords: Vec::new(),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARE_URL, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), MAX_FILES, "the listing is cut at the limit");
    assert_eq!(
        host.seen.lock().expect("seen").len(),
        1,
        "and the walk stops rather than reading on"
    );
    assert_eq!(links[0].file_name.as_deref(), Some("f0.bin"));
    assert_eq!(
        links[MAX_FILES - 1].file_name.as_deref(),
        Some("f499.bin"),
        "breadth first: what is kept is the near end, not a random half"
    );
}

// -- what is committed to the repository -----------------------------------------------------

/// Nothing that could be a credential or a real address is committed with these fixtures.
///
/// Nothing needs redacting today; this is what keeps it that way. A share token, a request
/// token and a host name are all things somebody copying an answer off a live instance would
/// bring along without noticing.
#[test]
fn fixtures_carry_no_credential_material() {
    /// The two share tokens the fixtures and this file are allowed to name. Both invented.
    const SHARE_TOKENS: [&str; 2] = ["QxT7bK2mNp9wZr4", "mK4pQ8vR2tY6nL0"];
    /// The request tokens the two Nextcloud pages carry. Both invented.
    const REQUEST_TOKENS: [&str; 2] = ["sBQ1cUZbW0E9V2xsVGZlQg==", "Rk9PYkFyMTIzNDU2Nzg5MA=="];
    /// A fixture may name a documentation domain and nothing else.
    const HOST_SUFFIXES: [&str; 3] = [".example.org", ".example.net", ".invalid"];
    /// The three schema namespaces a WebDAV document declares. They are identifiers, not
    /// addresses, and no request is ever made to them.
    const SCHEMA_HOSTS: [&str; 3] = ["sabredav.org", "nextcloud.org", "owncloud.org"];

    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nextcloud_crawler");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        for marker in ["/s/", "/dav/files/"] {
            for token in values_after(&body, marker) {
                assert!(
                    SHARE_TOKENS.contains(&token.as_str()),
                    "{path:?} carries a share token that is not a placeholder: {token}"
                );
            }
        }
        for token in attribute_values(&body, "data-requesttoken=\"") {
            assert!(
                REQUEST_TOKENS.contains(&token.as_str()),
                "{path:?} carries a request token that is not a placeholder: {token}"
            );
        }
        for authority in authorities(&body) {
            assert!(
                HOST_SUFFIXES
                    .iter()
                    .any(|suffix| authority.ends_with(suffix))
                    || SCHEMA_HOSTS.contains(&authority.as_str()),
                "{path:?} names the host {authority}, which is not a documentation domain"
            );
        }
        // sabre/dav's own 401 message names the `Authorization: Basic` header, so the word
        // alone proves nothing. What must not be here is an encoded *value* after it.
        for marker in ["Basic ", "Bearer "] {
            for value in tokens_after(&body, marker) {
                assert!(
                    !credential_like(&value),
                    "{path:?} carries an encoded credential after `{marker}`: {value}"
                );
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 12, "the fixture directory holds twelve documents");
}

/// Every value following `marker`, up to the first character a token cannot contain.
fn values_after(text: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let tail = &rest[index + marker.len()..];
        let end = tail
            .find(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_')
            })
            .unwrap_or(tail.len());
        if end > 0 {
            out.push(tail[..end].to_owned());
        }
        rest = &tail[end..];
    }
    out
}

/// Every attribute value opened by `marker` and closed by the next double quote.
fn attribute_values(text: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let tail = &rest[index + marker.len()..];
        let Some(end) = tail.find('"') else { break };
        out.push(tail[..end].to_owned());
        rest = &tail[end + 1..];
    }
    out
}

/// Every `://<authority>` in `text`, lowercased.
fn authorities(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        let tail = &rest[index + 3..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '"' | '\'' | '<' | '>')
            })
            .unwrap_or(tail.len());
        out.push(tail[..end].to_ascii_lowercase());
        rest = &tail[end..];
    }
    out
}

/// Every whitespace-delimited token following `marker`.
fn tokens_after(text: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let tail = &rest[index + marker.len()..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>')
            })
            .unwrap_or(tail.len());
        out.push(tail[..end].to_owned());
        rest = &tail[end..];
    }
    out
}

/// Whether a token is an encoded value rather than a word: base64 and its relatives always
/// carry something that is not a letter, and a word never carries only those.
fn credential_like(value: &str) -> bool {
    value.len() >= 8
        && value
            .chars()
            .any(|character| character.is_ascii_digit() || matches!(character, '+' | '/' | '='))
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=' | '_' | '-')
        })
}

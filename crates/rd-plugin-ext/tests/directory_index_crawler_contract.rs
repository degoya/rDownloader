//! The generic directory-index crawler, exercised end to end against its built component.
//!
//! RD-107-05 wrote this plugin and tested its parser natively against real Apache and nginx
//! pages; what it could not show is whether the *guest* behaves in the host the way the host
//! expects. That is what this file is for. Everything here runs the real
//! `rd_plugin_directory_index_crawler.wasm` inside Wasmtime under the manifest the plugin
//! ships, so RD-107-05's second host gap is in the loop: the manifest declares `*` because an
//! open listing is any web server at all, `controlled_http_request` measures every address
//! against the list the host built for this one call, and a request arriving at the mock
//! therefore proves the narrowing admits the pasted host. Its other half is not proven here —
//! the sandbox's own refusal is stood in for by a host that fabricates
//! `plugin.http_target_not_allowed`, so what is asserted is that the refusal is reported
//! rather than turned into an empty listing, not that the allow-list produces it.
//!
//! **The server is a mock.** It answers at the host boundary, so no socket is opened and no
//! web server is contacted. A run against a live Apache or nginx is a separate acceptance
//! and is recorded in the job file, not claimed here.
//!
//! This plugin is the one that claims by the *shape* of an address, so it is wrong about one
//! sooner or later. Two tests exist only for that: a page that is not a listing and an
//! address that is not there both end as `unsupported`, which is the refusal
//! `FolderCrawlers::expand` walks past instead of ending the link on.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_host::{PluginManifest, extension::FolderCrawler};

const MANIFEST: &str = include_str!("../../../plugins/directory-index-crawler/manifest.toml");

const APACHE_INDEX: &str = include_str!("fixtures/directory_index_crawler/apache_index.html");
const APACHE_SUBDIRECTORY: &str =
    include_str!("fixtures/directory_index_crawler/apache_subdirectory.html");
const NGINX_INDEX: &str = include_str!("fixtures/directory_index_crawler/nginx_index.html");
const NGINX_SEASON: &str = include_str!("fixtures/directory_index_crawler/nginx_season.html");
const NGINX_EMPTY: &str = include_str!("fixtures/directory_index_crawler/nginx_empty.html");
const NGINX_LEVEL: &str = include_str!("fixtures/directory_index_crawler/nginx_level.html");
const NGINX_WIDE: &str = include_str!("fixtures/directory_index_crawler/nginx_wide.html");
const HOSTILE_INDEX: &str = include_str!("fixtures/directory_index_crawler/hostile_index.html");
const LANDING_PAGE: &str = include_str!("fixtures/directory_index_crawler/landing_page.html");
const FORBIDDEN: &str = include_str!("fixtures/directory_index_crawler/forbidden.html");
const UNAUTHORIZED: &str = include_str!("fixtures/directory_index_crawler/unauthorized.html");
const NOT_FOUND: &str = include_str!("fixtures/directory_index_crawler/not_found.html");

/// The nginx mirror the fixtures were written around.
const MIRROR: &str = "https://files.example.org/mirror/";
/// The Apache release directory the fixtures were written around.
const RELEASES: &str = "https://files.example.org/pub/releases/";

/// The bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-directory-index-crawler")
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

/// A web server with an open directory listing, answering at the host boundary.
struct MockWebServer {
    /// Path to the status and body served for it.
    pages: Vec<(String, (u16, String))>,
    /// What every unlisted path answers; nginx's own 404 page by default.
    fallback: (u16, String),
    seen: Mutex<Vec<Seen>>,
}

impl MockWebServer {
    /// A server serving these listings, and a 404 everywhere else.
    fn serving(pages: Vec<(&str, &str)>) -> Arc<Self> {
        Arc::new(Self {
            pages: pages
                .into_iter()
                .map(|(path, body)| (path.to_owned(), (200, body.to_owned())))
                .collect(),
            fallback: (404, NOT_FOUND.to_owned()),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// A server that answers every address with one status and one document.
    fn answering(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            pages: Vec::new(),
            fallback: (status, body.to_owned()),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// A server whose listings are as `serving`, plus paths it refuses outright.
    fn serving_and_refusing(pages: Vec<(&str, &str)>, refused: Vec<(&str, u16)>) -> Arc<Self> {
        let mut all: Vec<(String, (u16, String))> = pages
            .into_iter()
            .map(|(path, body)| (path.to_owned(), (200, body.to_owned())))
            .collect();
        all.extend(
            refused
                .into_iter()
                .map(|(path, status)| (path.to_owned(), (status, FORBIDDEN.to_owned()))),
        );
        Arc::new(Self {
            pages: all,
            fallback: (404, NOT_FOUND.to_owned()),
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
}

#[async_trait]
impl ResolverHost for MockWebServer {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.seen.lock().expect("seen").push(Seen {
            method: request.method.clone(),
            path: request.url.path().to_owned(),
            headers: request
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value_template.clone()))
                .collect(),
        });
        let (status, body) = self
            .pages
            .iter()
            .find(|(path, _)| path == request.url.path())
            .map_or_else(|| self.fallback.clone(), |(_, answer)| answer.clone());
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

fn crawler(host: Arc<MockWebServer>, bytes: &[u8]) -> FolderCrawler {
    FolderCrawler::new(manifest(), bytes, Some(host)).expect("compile the crawler")
}

/// A crawler is asked before it is handed anything, and it answers from the address alone.
///
/// This one is asked about *every* address a person pastes and is asked last, after every
/// crawler that names a service — so an answer that needed a request would cost the whole
/// queue one.
#[tokio::test]
async fn only_addresses_that_end_in_a_slash_are_claimed_and_deciding_reaches_nothing() {
    let bytes = component();
    let host = MockWebServer::serving(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [MIRROR, RELEASES, "http://files.example.org/"] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for foreign in [
        // A file: somebody else resolves it.
        "https://files.example.org/mirror/disc.iso",
        // A page with a query does something; it is not a directory.
        "https://files.example.org/mirror/?C=M;O=D",
        "ftp://files.example.org/mirror/",
        "magnet:?xt=urn:btih:0123456789abcdef",
    ] {
        assert!(!crawler.claims(foreign).await.expect("claims"), "{foreign}");
    }
    assert!(
        host.paths().is_empty(),
        "deciding whether a link is claimed must reach nothing"
    );
}

/// An Apache index becomes its files and follows its subdirectory, and neither the link back
/// up nor Apache's four sort links become entries.
///
/// Apache prints sizes for people (`1.2K`), so none is claimed here: a missing size costs
/// nothing because the transfer learns the real one, whereas a guessed one would be shown to
/// somebody as a fact.
#[tokio::test]
async fn an_apache_index_becomes_its_files_and_its_subdirectory() {
    let bytes = component();
    let host = MockWebServer::serving(vec![
        ("/pub/releases/", APACHE_INDEX),
        ("/pub/releases/24.04/", APACHE_SUBDIRECTORY),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(RELEASES, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 4, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("CHECKSUMS.txt"));
    assert_eq!(
        links[0].url,
        "https://files.example.org/pub/releases/CHECKSUMS.txt"
    );
    assert_eq!(links[0].package_hint.as_deref(), Some("releases"));
    assert_eq!(
        links[0].size, None,
        "Apache prints 1.2K, which is not a count"
    );
    assert_eq!(links[1].file_name.as_deref(), Some("release notes.pdf"));
    assert_eq!(
        links[1].url, "https://files.example.org/pub/releases/release%20notes.pdf",
        "the address stays percent-encoded: it is what the transfer will request"
    );
    assert_eq!(links[2].file_name.as_deref(), Some("server-24.04.iso"));
    assert_eq!(
        links[2].package_hint.as_deref(),
        Some("releases/24.04"),
        "a subdirectory extends the package name rather than replacing it"
    );
    assert_eq!(links[3].file_name.as_deref(), Some("SHA256SUMS"));

    assert_eq!(
        host.paths(),
        vec![
            "/pub/releases/".to_owned(),
            "/pub/releases/24.04/".to_owned()
        ],
        "the parent link and the sort links were never fetched"
    );
}

/// An nginx index becomes its files with the byte counts nginx actually prints, and its two
/// subdirectories are read — including the one that turns out to be empty.
#[tokio::test]
async fn an_nginx_index_becomes_its_files_with_the_counts_it_prints() {
    let bytes = component();
    let host = MockWebServer::serving(vec![
        ("/mirror/", NGINX_INDEX),
        ("/mirror/Season%201/", NGINX_SEASON),
        ("/mirror/incoming/", NGINX_EMPTY),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 4, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("README"));
    assert_eq!(links[0].size, Some(2048));
    assert_eq!(links[0].package_hint.as_deref(), Some("mirror"));
    assert_eq!(links[1].file_name.as_deref(), Some("disc.iso"));
    assert_eq!(links[1].size, Some(1_048_576));
    assert_eq!(links[2].file_name.as_deref(), Some("e01.mkv"));
    assert_eq!(links[2].size, Some(734_003_200));
    assert_eq!(links[2].package_hint.as_deref(), Some("mirror/Season 1"));
    assert_eq!(
        links[2].url,
        "https://files.example.org/mirror/Season%201/e01.mkv"
    );
    assert_eq!(links[3].file_name.as_deref(), Some("e02.mkv"));

    assert_eq!(
        host.paths(),
        vec![
            "/mirror/".to_owned(),
            "/mirror/Season%201/".to_owned(),
            "/mirror/incoming/".to_owned()
        ],
        "breadth first, and an empty subdirectory is still a subdirectory"
    );
}

/// The listing is fetched with a browser-shaped `Accept`, because a server that
/// content-negotiates answers a bare request with something that is not the listing.
#[tokio::test]
async fn the_listing_is_a_get_with_the_accept_a_negotiating_server_needs() {
    let bytes = component();
    let host = MockWebServer::serving(vec![("/mirror/", NGINX_EMPTY)]);
    crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect_err("the fixture is an empty listing");

    let request = host.first();
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.header("Accept"),
        Some("text/html,application/xhtml+xml")
    );
    assert_eq!(
        request.header("Authorization"),
        None,
        "an open listing is open: no credential is sent"
    );
}

/// A tree deeper than the plugin walks stops at its own limit rather than following a
/// stranger's directory structure to the bottom.
///
/// The limit belongs to the plugin (`walk::MAX_DEPTH`), and asserting it against the
/// component rather than in the plugin's own tests is the point: a guest that ignored its
/// own bookkeeping would make a request per level for ever, and every one of those requests
/// looks perfectly reasonable on its own.
#[tokio::test]
async fn a_tree_deeper_than_the_limit_stops_at_the_limit() {
    let bytes = component();
    let mut pages: Vec<(String, (u16, String))> = Vec::new();
    for level in 0..8u32 {
        let own = format!("/mirror/{}", "deeper/".repeat(level as usize));
        pages.push((
            own.clone(),
            (
                200,
                NGINX_LEVEL
                    .replace("__SELF__", &own)
                    .replace("__LEVEL__", &level.to_string()),
            ),
        ));
    }
    let host = Arc::new(MockWebServer {
        pages,
        fallback: (404, NOT_FOUND.to_owned()),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect("a listing");

    // The crawled directory plus four levels below it: five read, five files found.
    assert_eq!(host.paths().len(), 5, "{:?}", host.paths());
    assert_eq!(links.len(), 5, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("note0.bin"));
    assert_eq!(links[4].file_name.as_deref(), Some("note4.bin"));
    assert_eq!(
        links[4].package_hint.as_deref(),
        Some("mirror/deeper/deeper/deeper/deeper")
    );
}

/// A page a stranger controls cannot point the crawl anywhere but under the address it was
/// given: not at another host, not further up, not three levels down, and not at a page with
/// a query string.
///
/// The parser drops all of them, and the sandbox would refuse the off-host ones underneath —
/// two independent guards. What this asserts is the first: nothing was even attempted.
#[tokio::test]
async fn a_listing_cannot_point_the_crawl_off_the_address_it_was_given() {
    let bytes = component();
    let host = MockWebServer::serving(vec![("/mirror/", HOSTILE_INDEX)]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("good.bin"));
    assert_eq!(links[0].url, "https://files.example.org/mirror/good.bin");
    assert_eq!(
        host.paths(),
        vec!["/mirror/".to_owned()],
        "nothing outside the crawled path was fetched"
    );
}

/// A subdirectory that is refused is a hole in the tree, not the end of the crawl — but the
/// address a person actually pasted being refused *is* the end of it.
#[tokio::test]
async fn a_refused_subdirectory_costs_only_that_subdirectory() {
    let bytes = component();
    let host = MockWebServer::serving_and_refusing(
        vec![
            ("/mirror/", NGINX_INDEX),
            ("/mirror/incoming/", NGINX_EMPTY),
        ],
        vec![("/mirror/Season%201/", 403)],
    );
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(
        links.len(),
        2,
        "the two files of the listing itself: {links:?}"
    );
    assert_eq!(links[0].file_name.as_deref(), Some("README"));
    assert_eq!(
        host.paths().len(),
        3,
        "the refused directory was still tried"
    );

    // The same refusal on the crawled address itself ends the crawl, with its own code.
    let refusing = MockWebServer::answering(403, FORBIDDEN);
    let refusal = crawler(refusing, &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect_err("a refused listing is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("directory_index_crawler.sign_in_required")
    );
    assert!(
        !refusal.not_mine,
        "the address is a directory; it is closed"
    );
}

/// The three addresses the job names — empty, locked, missing — each end with their own
/// stable code rather than with an empty list or a line nobody can act on.
#[tokio::test]
async fn an_empty_a_locked_and_a_missing_listing_each_end_with_their_own_code() {
    let bytes = component();

    // Empty: the listing exists, the server answered, and there is nothing in it.
    let empty = MockWebServer::serving(vec![("/mirror/", NGINX_EMPTY)]);
    let refusal = crawler(empty, &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect_err("an empty listing is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("directory_index_crawler.directory_empty")
    );
    assert!(!refusal.not_mine, "the address is a directory; it is empty");

    // Locked: the server wants a sign-in, which this plugin has nothing to offer.
    for status in [401u16, 403] {
        let body = if status == 401 {
            UNAUTHORIZED
        } else {
            FORBIDDEN
        };
        let locked = MockWebServer::answering(status, body);
        let refusal = crawler(locked, &bytes)
            .crawl(MIRROR, None)
            .await
            .expect("call")
            .expect_err("a locked listing is a refusal");
        assert_eq!(
            refusal.code.as_deref(),
            Some("directory_index_crawler.sign_in_required"),
            "status {status}"
        );
    }

    // Missing: there was never a directory here, so the address goes on to the next crawler
    // rather than ending. This is RD-107-05's third host gap, end to end.
    let missing = MockWebServer::serving(Vec::new());
    let refusal = crawler(missing, &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect_err("a missing listing is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("directory_index_crawler.not_a_listing")
    );
    assert!(
        refusal.not_mine,
        "a wrong guess hands the address on rather than ending the link"
    );
}

/// A page that merely ends in a slash and has links on it is not a listing, and saying so is
/// this plugin's whole survival strategy: it claims by shape, so it is wrong sooner or later,
/// and `unsupported` is what keeps a wrong guess from ending the link.
#[tokio::test]
async fn a_page_that_is_not_a_listing_hands_the_address_to_the_next_crawler() {
    let bytes = component();
    let host = MockWebServer::serving(vec![("/mirror/", LANDING_PAGE)]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect_err("a landing page is not a listing");

    assert_eq!(
        refusal.code.as_deref(),
        Some("directory_index_crawler.not_a_listing")
    );
    assert!(refusal.not_mine);
    assert_eq!(
        host.paths(),
        vec!["/mirror/".to_owned()],
        "the page was read once and none of its links was followed"
    );
}

/// A busy server and one that is broken are two different answers, and each says which.
#[tokio::test]
async fn a_busy_server_and_a_broken_one_are_told_apart() {
    let bytes = component();
    for (status, code) in [
        (429u16, "directory_index_crawler.server_busy"),
        (503, "directory_index_crawler.directory_unreachable"),
    ] {
        let host = MockWebServer::answering(status, "");
        let refusal = crawler(host, &bytes)
            .crawl(MIRROR, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(refusal.code.as_deref(), Some(code), "status {status}");
        assert!(!refusal.not_mine, "status {status}");
    }
}

/// A crawler reaches nothing the host did not grant it for this one call.
///
/// The manifest says `*`; what applies is the host of the pasted address, and a request
/// anywhere else is refused before a socket exists. The refusing host here is a stub that
/// answers with the code the sandbox would answer with — so what this asserts is the guest's
/// half, that the refusal is carried out rather than turned into an empty listing. That the
/// allow-list produces the code is `rd-plugin-host`'s own test.
#[tokio::test]
async fn a_request_the_sandbox_refuses_is_reported_and_not_turned_into_an_empty_listing() {
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
        .crawl(MIRROR, None)
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
async fn an_address_that_is_not_a_directory_is_handed_on_without_a_request() {
    let bytes = component();
    let host = MockWebServer::serving(vec![("/mirror/", NGINX_INDEX)]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl("https://files.example.org/mirror/disc.iso", None)
        .await
        .expect("call")
        .expect_err("not a directory");
    assert_eq!(
        refusal.code.as_deref(),
        Some("directory_index_crawler.not_a_directory")
    );
    assert!(refusal.not_mine);
    assert!(host.paths().is_empty(), "nothing was fetched");
}

/// This crawler is asked last. Nothing in this file would notice if that changed, so the
/// manifest flag the ordering rests on is asserted here rather than left to a comment.
#[test]
fn the_manifest_declares_this_crawler_generic_so_it_is_asked_last() {
    let extension = manifest().extension.expect("the extension block");
    assert!(
        extension.generic,
        "a crawler claiming by shape must be asked after every crawler that names a service"
    );
}

/// How many directories one crawl of this plugin reads, and how many files it hands back.
///
/// The numbers belong to `plugins/directory-index-crawler/src/walk.rs` (`MAX_DIRECTORIES`,
/// `MAX_FILES`) and are repeated here rather than imported: a contract test asserts what the
/// component does, and a constant shared with the code under test would move with it.
const MAX_DIRECTORIES: usize = 100;
const MAX_FILES: usize = 500;

/// One listing row for a subdirectory, spaced the way nginx spaces one.
fn wide_directory(index: usize) -> String {
    format!(
        "<a href=\"n{index}/\">n{index}/</a>{:width$}27-Mar-2024 15:24                   -",
        "",
        width = 40usize.saturating_sub(index.to_string().len())
    )
}

/// One listing row for a file, with the byte count nginx prints at the end of one.
fn wide_file(index: usize) -> String {
    format!(
        "<a href=\"f{index}.bin\">f{index}.bin</a>{:width$}27-Mar-2024 15:24                1024",
        "",
        width = 36usize.saturating_sub(index.to_string().len())
    )
}

/// The wide-listing envelope filled with `entries`.
fn wide_listing(path: &str, entries: &[String]) -> String {
    NGINX_WIDE
        .replace("__SELF__", path)
        .replace("__ENTRIES__", &format!("{}\n", entries.join("\n")))
}

/// A listing wider than the plugin reads stops at its own breadth limits rather than
/// following a stranger's directory however far it goes sideways.
///
/// The depth test covers the tree going down; this one covers it going across, which is the
/// cheaper thing for a stranger to arrange: one directory holding a hundred thousand names
/// costs one page to serve and a hundred thousand requests to obey. Both limits are asserted
/// at the component, because a guest that ignored its own bookkeeping would make every one of
/// those requests and each one would look perfectly reasonable.
#[tokio::test]
async fn a_listing_wider_than_the_limits_stops_at_them() {
    let bytes = component();

    // Half again as many subdirectories as the walk is allowed to read.
    let siblings: Vec<String> = (0..150).map(wide_directory).collect();
    let mut pages: Vec<(String, (u16, String))> = vec![(
        "/mirror/".to_owned(),
        (200, wide_listing("/mirror/", &siblings)),
    )];
    for index in 0..150usize {
        let own = format!("/mirror/n{index}/");
        pages.push((
            own.clone(),
            (
                200,
                NGINX_LEVEL
                    .replace("__SELF__", &own)
                    .replace("__LEVEL__", &index.to_string()),
            ),
        ));
    }
    let host = Arc::new(MockWebServer {
        pages,
        fallback: (404, NOT_FOUND.to_owned()),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(
        host.paths().len(),
        MAX_DIRECTORIES,
        "the crawled directory and ninety-nine of its subdirectories, and not one more"
    );
    assert_eq!(
        links.len(),
        MAX_DIRECTORIES - 1,
        "one file out of each subdirectory that was read"
    );

    // And the file limit, which a single directory can reach on its own.
    let many: Vec<String> = (0..600).map(wide_file).collect();
    let host = Arc::new(MockWebServer {
        pages: vec![(
            "/mirror/".to_owned(),
            (200, wide_listing("/mirror/", &many)),
        )],
        fallback: (404, NOT_FOUND.to_owned()),
        seen: Mutex::new(Vec::new()),
    });
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(MIRROR, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), MAX_FILES, "the listing is cut at the limit");
    assert_eq!(
        host.paths().len(),
        1,
        "and the walk stops rather than reading on"
    );
    assert_eq!(links[0].file_name.as_deref(), Some("f0.bin"));
    assert_eq!(links[0].size, Some(1024));
    assert_eq!(
        links[MAX_FILES - 1].file_name.as_deref(),
        Some("f499.bin"),
        "breadth first: what is kept is the near end, not a random half"
    );
}

// -- what is committed to the repository -----------------------------------------------------

/// Nothing that could be a credential or a real address is committed with these fixtures.
///
/// Nothing needs redacting today; this is what keeps it that way. An open directory listing
/// is a page off somebody's server, and a host name is exactly what copying one brings along.
#[test]
fn fixtures_carry_no_credential_material() {
    /// A fixture may name a documentation domain and nothing else. `.invalid` is reserved and
    /// resolves nowhere, which is what the hostile-listing fixture needs its off-site link to
    /// be.
    const HOST_SUFFIXES: [&str; 3] = [".example.org", ".example.net", ".invalid"];

    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/directory_index_crawler");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        for authority in authorities(&body) {
            assert!(
                HOST_SUFFIXES
                    .iter()
                    .any(|suffix| authority.ends_with(suffix)),
                "{path:?} names the host {authority}, which is not a documentation domain"
            );
        }
        // An open listing carries no credential at all, and no fixture here may either.
        for word in ["Basic ", "Bearer ", "Authorization:", "Set-Cookie"] {
            assert!(
                !body.contains(word),
                "{path:?} carries a `{word}` value; a credential belongs in the test, not here"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 12, "the fixture directory holds twelve documents");
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

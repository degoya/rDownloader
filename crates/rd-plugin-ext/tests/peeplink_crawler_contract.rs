//! The PEEPLink entry crawler, exercised end to end against its built component (RD-110-17).
//!
//! The plugin's own crate tests the address decision and the page reader natively against the
//! seven pages recorded on 2026-09-21; this file runs the real
//! `rd_plugin_peeplink_crawler.wasm` inside Wasmtime under the manifest the plugin ships.
//!
//! **The service is a mock.** It answers at the host boundary, so no socket is opened and
//! neither `peeplink.in` nor `alfalink.to` is contacted. What it answers with are the recorded
//! pages in `plugins/peeplink-crawler/tests/fixtures/` (see the README there); the refusals it
//! adds around them — a `500`, a `429`, a page that is not of this service — are generated and
//! labelled as such. The cases: the recorded entries in both HTML shapes (24, 1, 9 and 14 links
//! from one `GET`, nothing followed); a `404` and a deleted entry answering `200` at the front
//! page, both `entry_not_found` and neither *not mine*; an `<article>` naming nothing,
//! `entry_empty`; a `500`, a `429` and a foreign page, `site_unreachable`; and an unclaimed
//! address, `unsupported`, which the selection walks past.
//!
//! **Not here: the password branch.** No protected entry was findable, so there is no page to
//! record, and the job file says plainly that the branch stays untested until one is. Adding a
//! fabricated protected page here would turn "untested" into a green number.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_host::{PluginManifest, extension::FolderCrawler};

const MANIFEST: &str = include_str!("../../../plugins/peeplink-crawler/manifest.toml");

const PEEPLINK_WIDE: &str =
    include_str!("../../../plugins/peeplink-crawler/tests/fixtures/peeplink-0004ae96cef6.html");
const PEEPLINK_SINGLE: &str =
    include_str!("../../../plugins/peeplink-crawler/tests/fixtures/peeplink-00013b965394.html");
const PEEPLINK_NOT_FOUND: &str =
    include_str!("../../../plugins/peeplink-crawler/tests/fixtures/peeplink-unknown-404.html");
const PEEPLINK_FRONT_PAGE: &str =
    include_str!("../../../plugins/peeplink-crawler/tests/fixtures/peeplink-deleted-redirect.html");
const ALFALINK_NINE: &str = include_str!(
    "../../../plugins/peeplink-crawler/tests/fixtures/alfalink-02489255ba1048ae9d1328.html"
);
const ALFALINK_FOURTEEN: &str = include_str!(
    "../../../plugins/peeplink-crawler/tests/fixtures/alfalink-13e2cd9a35efcd6c6e4766.html"
);
const ALFALINK_NOT_FOUND: &str =
    include_str!("../../../plugins/peeplink-crawler/tests/fixtures/alfalink-unknown-404.html");

const CATALOGUE: &str = include_str!("../../../plugins/peeplink-crawler/locales/en.json");

const WIDE: &str = "https://peeplink.in/0004ae96cef6";
const SINGLE: &str = "https://peeplink.in/00013b965394";
const NINE: &str = "https://alfalink.to/02489255ba1048ae9d1328";
const FOURTEEN: &str = "https://alfalink.to/13e2cd9a35efcd6c6e4766";

/// The bundled component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-peeplink-crawler")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the crawler manifest")
}

/// One answer the mock is set up to give.
#[derive(Clone)]
struct Answer {
    status: u16,
    body: String,
    /// Where the answer came from after redirects; the request's own address when `None`.
    final_url: Option<String>,
}

impl Answer {
    fn page(body: &str) -> Self {
        Self {
            status: 200,
            body: body.to_owned(),
            final_url: None,
        }
    }

    fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_owned(),
            final_url: None,
        }
    }

    /// A deleted entry: `200`, but the answer came from the front page.
    fn redirected_to(final_url: &str, body: &str) -> Self {
        Self {
            status: 200,
            body: body.to_owned(),
            final_url: Some(final_url.to_owned()),
        }
    }
}

/// One request the plugin made, as the host saw it.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    url: String,
}

/// The link-protection service, answering at the host boundary.
struct MockService {
    answers: Vec<(String, Answer)>,
    fallback: Answer,
    seen: Mutex<Vec<Seen>>,
}

impl MockService {
    /// A service serving these addresses, and the recorded `404` page everywhere else.
    fn serving(answers: Vec<(&str, Answer)>) -> Arc<Self> {
        Arc::new(Self {
            answers: answers
                .into_iter()
                .map(|(url, answer)| (url.to_owned(), answer))
                .collect(),
            fallback: Answer::status(404, PEEPLINK_NOT_FOUND),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// A service that answers every address the same way.
    fn answering(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answers: Vec::new(),
            fallback: answer,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().expect("seen").clone()
    }
}

#[async_trait]
impl ResolverHost for MockService {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let url = request.url.to_string();
        self.seen.lock().expect("seen").push(Seen {
            method: request.method.clone(),
            url: url.clone(),
        });
        let answer = self
            .answers
            .iter()
            .find(|(known, _)| *known == url.trim_end_matches('/'))
            .map_or_else(|| self.fallback.clone(), |(_, answer)| answer.clone());
        Ok(HostHttpResponse {
            status: answer.status,
            final_url: match answer.final_url {
                Some(moved) => moved.parse().unwrap_or_else(|_| request.url.clone()),
                None => request.url.clone(),
            },
            headers: Vec::new(),
            body: answer.body.into_bytes(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

fn crawler(host: Arc<MockService>, bytes: &[u8]) -> FolderCrawler {
    FolderCrawler::new(manifest(), bytes, Some(host)).expect("compile the crawler")
}

/// A crawler is asked before it is handed anything, and it answers from the address alone.
#[tokio::test]
async fn only_entry_addresses_of_the_two_live_domains_are_claimed_and_deciding_reaches_nothing() {
    let bytes = component();
    let host = MockService::serving(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);

    for claimed in [
        WIDE,
        SINGLE,
        NINE,
        FOURTEEN,
        "https://www.peeplink.in/0004ae96cef6",
        "http://alfalink.to/02489255ba1048ae9d1328",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for left_alone in [
        // The service's own pages are not entries.
        "https://peeplink.in/",
        "https://peeplink.in/tos.html",
        "https://alfalink.to/login.php",
        // The alias that timed out on both measuring days: recognised, never claimed, because
        // the manifest grants no request to it.
        "https://alfalink.info/02489255ba1048ae9d1328",
        // Somebody else's address in the same shape.
        "https://example.org/0004ae96cef6",
        "https://filecrypt.cc/Container/ABCDEF0123.html",
    ] {
        assert!(
            !crawler.claims(left_alone).await.expect("claims"),
            "{left_alone}"
        );
    }
    assert!(host.requests().is_empty(), "claiming reaches nothing");
}

/// The recorded entry, through the real component: 24 links, one request, nothing followed.
#[tokio::test]
async fn the_recorded_entry_yields_its_twenty_four_links_from_one_request() {
    let bytes = component();
    let host = MockService::serving(vec![(WIDE, Answer::page(PEEPLINK_WIDE))]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(WIDE, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 24, "{links:?}");
    assert_eq!(
        links[0].url,
        "https://rapidgator.net/file/9f3a418df007e8fb8ac2b8f3a62c46d8/227weji-thatt-macau.part1.rar.html"
    );
    // The entry page names addresses and nothing else; the hoster owns the name and the size.
    assert!(
        links
            .iter()
            .all(|link| link.file_name.is_none() && link.size.is_none()),
        "{links:?}"
    );
    let requests = host.requests();
    assert_eq!(requests.len(), 1, "one GET and nothing else: {requests:?}");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url, WIDE);
}

/// The narrow end of the same shape, and the other domain's shape beside it.
#[tokio::test]
async fn the_single_link_entry_and_both_alfalink_entries_come_back_whole() {
    let bytes = component();
    let host = MockService::serving(vec![
        (SINGLE, Answer::page(PEEPLINK_SINGLE)),
        (NINE, Answer::page(ALFALINK_NINE)),
        (FOURTEEN, Answer::page(ALFALINK_FOURTEEN)),
    ]);
    let crawler = crawler(Arc::clone(&host), &bytes);

    let single = crawler
        .crawl(SINGLE, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].url, "http://uploaded.net/file/zr53zjoh");

    // `<article class="articless">` with the addresses as bare text between `<br/>` tags.
    let nine = crawler
        .crawl(NINE, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(nine.len(), 9, "{nine:?}");
    assert_eq!(nine[0].url, "https://streamtape.com/v/bGVqRGBmXKtPVoQ");

    let fourteen = crawler
        .crawl(FOURTEEN, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(fourteen.len(), 14, "{fourteen:?}");
    assert!(
        fourteen
            .iter()
            .all(|link| !link.url.contains("peeplink.in") && !link.url.contains("alfalink.")),
        "the service's own addresses never come back as downloads: {fourteen:?}"
    );
}

/// An identifier the service never knew answers `404`, on both domains. Reported as a fact
/// about the entry, not as "not mine": the address is on the service's domain and in its
/// shape, so handing it on would replace an honest refusal with a vaguer one.
#[tokio::test]
async fn an_unknown_identifier_ends_as_entry_not_found_rather_than_as_not_mine() {
    let bytes = component();
    for (address, page) in [
        ("https://peeplink.in/aaaaaaaaaaaa", PEEPLINK_NOT_FOUND),
        (
            "https://alfalink.to/aaaaaaaaaaaaaaaaaaaaaa",
            ALFALINK_NOT_FOUND,
        ),
    ] {
        let host = MockService::answering(Answer::status(404, page));
        let refusal = crawler(Arc::clone(&host), &bytes)
            .crawl(address, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(
            refusal.code.as_deref(),
            Some("peeplink_crawler.entry_not_found"),
            "{address}"
        );
        assert!(!refusal.not_mine, "{address}");
    }
}

/// A deleted entry answers `200` at the front page, so the status alone cannot tell. Without
/// the address check this is the worst of the four refusals: the front page carries the
/// service's own submission form, and reading it would report an empty entry.
#[tokio::test]
async fn a_deleted_entry_redirected_to_the_front_page_ends_as_entry_not_found() {
    let bytes = component();
    let deleted = "https://peeplink.in/0005738dc976";
    let host = MockService::serving(vec![(
        deleted,
        Answer::redirected_to("https://peeplink.in/", PEEPLINK_FRONT_PAGE),
    )]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(deleted, None)
        .await
        .expect("call")
        .expect_err("a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("peeplink_crawler.entry_not_found")
    );
    assert!(!refusal.not_mine);
}

/// The service's own `www.` redirect keeps the entry, and must not read as a deletion.
#[tokio::test]
async fn the_redirect_from_the_www_host_to_the_bare_one_is_not_a_deletion() {
    let bytes = component();
    let pasted = "https://www.peeplink.in/0004ae96cef6";
    let host = MockService::serving(vec![(pasted, Answer::redirected_to(WIDE, PEEPLINK_WIDE))]);
    let crawler = crawler(Arc::clone(&host), &bytes);
    let links = crawler
        .crawl(pasted, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 24);
}

/// An entry that exists and names nothing is its own refusal, never an empty package. Not
/// recorded but generated, because all four measured entries carried links: this is the
/// single-link entry with its one address taken out.
#[tokio::test]
async fn an_entry_whose_article_names_nothing_ends_as_entry_empty() {
    let bytes = component();
    let emptied = PEEPLINK_SINGLE.replace("http://uploaded.net/file/zr53zjoh", "no links today");
    let host = MockService::serving(vec![(SINGLE, Answer::page(&emptied))]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(SINGLE, None)
        .await
        .expect("call")
        .expect_err("a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("peeplink_crawler.entry_empty")
    );
    assert!(!refusal.not_mine);
}

/// Everything that is not an entry page and not a `404` is one code: the service is not
/// answering. Generated answers, all three of them.
#[tokio::test]
async fn a_server_error_a_rate_limit_and_a_foreign_page_all_end_as_site_unreachable() {
    let bytes = component();
    for (label, answer) in [
        (
            "a 500",
            Answer::status(500, "<h1>Internal Server Error</h1>"),
        ),
        ("a 429", Answer::status(429, "<h1>Too Many Requests</h1>")),
        // A `200` that is not a page of this service at all: no `<article>` anywhere.
        (
            "a page of some other site",
            Answer::page("<html><body><p>parked</p></body></html>"),
        ),
    ] {
        let host = MockService::answering(answer);
        let refusal = crawler(Arc::clone(&host), &bytes)
            .crawl(WIDE, None)
            .await
            .expect("call")
            .expect_err("a refusal");
        assert_eq!(
            refusal.code.as_deref(),
            Some("peeplink_crawler.site_unreachable"),
            "{label}"
        );
        assert!(!refusal.not_mine, "{label}");
    }
}

/// The defensive case: the host asks `claims_url` first, but a crawl of an address this plugin
/// does not claim says `unsupported`, which the selection walks past.
#[tokio::test]
async fn an_address_this_plugin_does_not_claim_is_handed_on_rather_than_ended() {
    let bytes = component();
    let host = MockService::serving(Vec::new());
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl("https://example.org/somewhere/else", None)
        .await
        .expect("call")
        .expect_err("a refusal");
    assert!(refusal.not_mine, "{refusal:?}");
    assert!(
        host.requests().is_empty(),
        "a refused address is never fetched"
    );
}

/// The manifest is the whole grant: two live domains, no `captcha`, `cookies` or account.
#[test]
fn the_manifest_grants_the_two_live_domains_and_nothing_else() {
    let manifest = manifest();
    let domains = manifest
        .capabilities
        .net_http
        .as_ref()
        .expect("net_http")
        .domains
        .clone();
    assert_eq!(
        domains,
        vec![
            "peeplink.in".to_owned(),
            "www.peeplink.in".to_owned(),
            "alfalink.to".to_owned(),
            "www.alfalink.to".to_owned(),
        ]
    );
    // The alias that no longer answers is not granted.
    assert!(
        !domains
            .iter()
            .any(|domain| domain.contains("alfalink.info"))
    );
    assert!(
        manifest.capabilities.secrets.is_empty(),
        "an entry page is public"
    );
    // The three sitekeys on those pages belong to the login and register popups, and this
    // plugin never signs in — so neither grant is asked for and there is no captcha branch.
    assert!(!manifest.capabilities.captcha, "no captcha branch exists");
    assert!(
        !manifest.capabilities.cookies,
        "the page is not session bound"
    );
}

/// The recorded pages carry nothing that had to be redacted; asserted rather than promised.
#[test]
fn the_fixtures_carry_no_cookie_session_or_address_of_whoever_recorded_them() {
    for (name, page) in [
        ("wide", PEEPLINK_WIDE),
        ("single", PEEPLINK_SINGLE),
        ("peeplink 404", PEEPLINK_NOT_FOUND),
        ("front page", PEEPLINK_FRONT_PAGE),
        ("alfalink nine", ALFALINK_NINE),
        ("alfalink fourteen", ALFALINK_FOURTEEN),
        ("alfalink 404", ALFALINK_NOT_FOUND),
    ] {
        let lowered = page.to_ascii_lowercase();
        for forbidden in ["set-cookie", "phpsessid", "csrf", "authorization", "@gmail"] {
            assert!(!lowered.contains(forbidden), "{name} carries {forbidden}");
        }
        // The advertising loader, which differs on every request, is gone from both entry
        // pages of `peeplink.in`.
        assert!(!lowered.contains("popundersperip"), "{name}");
    }
}

/// A refusal is a code the interface can translate, never English prose alone.
#[tokio::test]
async fn every_refusal_carries_a_code_the_shipped_catalogues_translate() {
    let bytes = component();
    let catalogue: serde_json::Value = serde_json::from_str(CATALOGUE).expect("the catalogue");
    let host = MockService::answering(Answer::status(503, "<h1>down</h1>"));
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(WIDE, None)
        .await
        .expect("call")
        .expect_err("a refusal");
    let code = refusal.code.as_deref().expect("a code");
    assert!(catalogue["codes"][code].is_string(), "{code} untranslated");
    assert!(!refusal.message.is_empty());
}

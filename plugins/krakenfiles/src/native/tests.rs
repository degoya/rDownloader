//! The native resolver driven against the measured fixtures: a scripted host answers every
//! request from a queue and records what the plugin asked of it, so each test asserts what
//! the plugin *did* - which requests, which challenge, which headers - not only what it
//! returned. The flow itself is exercised in `flow_tests.rs`; this file holds the host, the
//! fixtures and the metadata cases.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind, LinkStatus};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};
use url::Url;

use super::KrakenfilesResolver;

/// The public file page as measured on 2026-09-21, token redacted (`tests/fixtures/README.md`).
pub(super) const FILE_PAGE: &str = include_str!("../../tests/fixtures/file-page-2026-09-21.html");
/// The site's 404 page for an id that does not exist.
pub(super) const ERROR_PAGE: &str = include_str!("../../tests/fixtures/error-page-2026-09-21.html");
/// `/json/<id>` for the live file.
pub(super) const JSON_FILE: &str = include_str!("../../tests/fixtures/json-file-2026-09-21.json");
/// `/json/<id>` for a deleted file: `[]`.
pub(super) const JSON_MISSING: &str =
    include_str!("../../tests/fixtures/json-missing-2026-09-21.json");
/// The measured answer to a post without a Turnstile answer, served under HTTP 500.
pub(super) const CAPTCHA_INVALID: &str =
    include_str!("../../tests/fixtures/download-captcha-invalid-2026-09-21.json");
/// The successful answer, **synthetic** - see the fixtures' README.
pub(super) const DOWNLOAD_OK: &str =
    include_str!("../../tests/fixtures/download-ok-synthetic.json");

pub(super) const TURNSTILE_SITE_KEY: &str = "0x4AAAAAAB4S-Cq-7quNHQy8";
pub(super) const FILE_PAGE_URL: &str = "https://krakenfiles.com/view/dp3ngkjnsx/file.html";
pub(super) const DIRECT_LINK: &str =
    "https://dl.krakenfiles.com/force-download/ZGlyZWN0LWxpbmstc3ludGhldGlj?fileHash=DP3nGKJNsX";

pub(super) struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    pub(super) requests: Mutex<Vec<HostHttpRequest>>,
    pub(super) captchas: Mutex<Vec<rd_plugin_api::CaptchaChallenge>>,
    /// Token every captcha is answered with; `None` mimics a host with no solver.
    captcha_token: Option<String>,
}

impl MockHost {
    pub(super) fn new(responses: Vec<HostHttpResponse>, captcha_token: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: captcha_token.map(str::to_owned),
        })
    }

    pub(super) fn request_count(&self) -> usize {
        self.requests.lock().expect("mock lock").len()
    }

    pub(super) fn request(&self, index: usize) -> HostHttpRequest {
        self.requests.lock().expect("mock lock")[index].clone()
    }

    pub(super) fn captcha_count(&self) -> usize {
        self.captchas.lock().expect("mock lock").len()
    }
}

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.lock().expect("mock lock").push(request);
        self.responses
            .lock()
            .expect("mock lock")
            .pop_front()
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "missing mock response"))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        challenge: rd_plugin_api::CaptchaChallenge,
        _limit: std::time::Duration,
    ) -> Result<rd_plugin_api::CaptchaAnswer, Failure> {
        self.captchas.lock().expect("mock lock").push(challenge);
        match &self.captcha_token {
            Some(token) => Ok(rd_plugin_api::CaptchaAnswer::Token(token.clone())),
            None => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            )),
        }
    }
}

pub(super) fn response(
    status: u16,
    final_url: &str,
    content_type: &str,
    body: &str,
) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: final_url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: content_type.to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

pub(super) fn html(status: u16, body: &str) -> HostHttpResponse {
    response(status, FILE_PAGE_URL, "text/html; charset=UTF-8", body)
}

pub(super) fn json(status: u16, body: &str) -> HostHttpResponse {
    response(
        status,
        "https://krakenfiles.com/download/DP3nGKJNsX",
        "application/json",
        body,
    )
}

/// The direct link's range probe: one byte of the file, its name and its length.
pub(super) fn file(status: u16) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: DIRECT_LINK.parse().expect("URL"),
        headers: vec![
            ResolvedHeader {
                name: "Content-Disposition".to_owned(),
                value: "attachment; filename=\"EldenRing_Fix_Repair_Steam_Generic.rar\"".to_owned(),
            },
            ResolvedHeader {
                name: "Content-Range".to_owned(),
                value: "bytes 0-0/5138022".to_owned(),
            },
        ],
        body: vec![0],
    }
}

pub(super) fn guest() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

pub(super) fn resolve_request(url: &str) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("URL"),
        client: guest(),
    }
}

pub(super) fn body_of(request: &HostHttpRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

pub(super) fn header_of(request: &HostHttpRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value_template.clone())
}

#[test]
fn the_resolver_requires_no_account_and_claims_the_url_table() {
    let resolver = KrakenfilesResolver::new(MockHost::new(Vec::new(), None));
    assert!(!resolver.metadata().requires_account);
    for link in [
        "https://krakenfiles.com/view/DP3nGKJNsX/file.html",
        "https://www.krakenfiles.com/view/DP3nGKJNsX/file.html",
        "http://krakenfiles.com/view/DP3NGKJNSX/file.html",
        "https://krakenfiles.com/embed-video/DP3nGKJNsX",
    ] {
        assert!(resolver.matches(&Url::parse(link).expect("url")), "{link}");
    }
    for link in [
        "https://krakenfiles.com/view/DP3nGKJNsX",
        "https://krakenfiles.com/DP3nGKJNsX",
        "https://krakenfiles.com/download/DP3nGKJNsX",
        "https://example.com/view/DP3nGKJNsX/file.html",
    ] {
        assert!(!resolver.matches(&Url::parse(link).expect("url")), "{link}");
    }
}

/// The manifest is the authority for the domains; the constant the transfer check consults
/// must say the same, or a link the manifest allows would be refused here (or the reverse).
#[test]
fn the_download_hosts_constant_matches_the_manifest() {
    let manifest: toml::Value = toml::from_str(crate::MANIFEST).expect("manifest parses");
    let declared: Vec<&str> = manifest["download_domains"]
        .as_array()
        .expect("download_domains")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    assert_eq!(declared, crate::page::DOWNLOAD_HOSTS);
    let matched: Vec<&str> = manifest["match_domains"]
        .as_array()
        .expect("match_domains")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    for hoster in crate::HOSTERS {
        assert!(matched.contains(hoster), "{hoster} is not a match domain");
    }
}

#[tokio::test]
async fn check_account_has_nothing_to_check() {
    let resolver = KrakenfilesResolver::new(MockHost::new(Vec::new(), None));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("no account to check");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("krakenfiles.no_account"));
}

#[tokio::test]
async fn hosters_is_the_apex_domain() {
    let resolver = KrakenfilesResolver::new(MockHost::new(Vec::new(), None));
    assert_eq!(
        resolver.hosters(AccountId::new()).await.expect("hosters"),
        vec!["krakenfiles.com".to_owned()]
    );
}

#[tokio::test]
async fn check_reads_the_metadata_endpoint_per_link() {
    let host = MockHost::new(
        vec![
            response(
                200,
                "https://krakenfiles.com/json/dp3ngkjnsx",
                "application/json",
                JSON_FILE,
            ),
            response(
                200,
                "https://krakenfiles.com/json/ztpdkgdzy8",
                "application/json",
                JSON_MISSING,
            ),
            response(
                200,
                "https://krakenfiles.com/json/other12345",
                "application/json",
                r#"{"title":"x","size":"1 KB","hash":"different00","url":""}"#,
            ),
            response(
                503,
                "https://krakenfiles.com/json/down123456",
                "text/html",
                "<title>503</title>",
            ),
        ],
        None,
    );
    let resolver = KrakenfilesResolver::new(host.clone());
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                "https://www.krakenfiles.com/view/DP3nGKJNsX/file.html"
                    .parse()
                    .expect("url"),
                "https://krakenfiles.com/embed-video/zTpdkgdZY8"
                    .parse()
                    .expect("url"),
                "https://krakenfiles.com/view/other12345/file.html"
                    .parse()
                    .expect("url"),
                "https://krakenfiles.com/view/down123456/file.html"
                    .parse()
                    .expect("url"),
                "https://example.com/view/DP3nGKJNsX/file.html"
                    .parse()
                    .expect("url"),
            ],
            client: guest(),
        })
        .await
        .expect("checked");
    assert_eq!(results.len(), 5);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(
        results[0].file_name.as_deref(),
        Some("EldenRing_Fix_Repair_Steam_Generic.rar")
    );
    assert_eq!(
        results[0].size.map(rd_core::ByteCount::get),
        Some(5_138_022),
        "4.90 MB, 1024-based"
    );
    assert_eq!(
        results[1].status,
        LinkStatus::Offline,
        "[] is a deleted file"
    );
    assert_eq!(
        results[2].status,
        LinkStatus::Offline,
        "a hash that is not the id"
    );
    assert_eq!(results[3].status, LinkStatus::Unknown, "the site was down");
    assert_eq!(
        results[4].status,
        LinkStatus::Unknown,
        "not a KrakenFiles link"
    );
    assert_eq!(
        host.request_count(),
        4,
        "one request per KrakenFiles link, none for the rest"
    );
    assert_eq!(
        host.request(0).url.as_str(),
        "https://krakenfiles.com/json/dp3ngkjnsx",
        "the lowercased id on the apex domain"
    );
    assert_eq!(
        host.request(1).url.as_str(),
        "https://krakenfiles.com/json/ztpdkgdzy8"
    );
}

#[path = "flow_tests.rs"]
mod flow_tests;

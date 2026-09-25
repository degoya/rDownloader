//! The native adapter against a scripted host: every URL form the job measured, the manifest's
//! promises, and the account-less contract. The resolve and check flows are in the sibling
//! files, on the same `MockHost`.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};
use url::Url;

use super::MediafireResolver;

/// The public file page, captured 2026-09-21 and sanitised.
pub(crate) const FILE_PAGE: &str = include_str!("../../tests/fixtures/file-page-2026-09-21.html");
pub(crate) const GET_INFO: &[u8] =
    include_bytes!("../../tests/fixtures/api-file-get-info-2026-09-21.json");
pub(crate) const GET_INFO_BATCH: &[u8] =
    include_bytes!("../../tests/fixtures/api-file-get-info-batch-2026-09-21.json");
pub(crate) const GET_INFO_INVALID: &[u8] =
    include_bytes!("../../tests/fixtures/api-file-get-info-invalid-2026-09-21.json");
pub(crate) const GET_INFO_MISSING: &[u8] =
    include_bytes!("../../tests/fixtures/api-file-get-info-missing-2026-09-21.json");
/// Synthetic: error 261 as documented.
pub(crate) const API_ERROR_261: &[u8] = include_bytes!("../../tests/fixtures/api-error-261.json");
pub(crate) const CAPTCHA_RECAPTCHA: &str =
    include_str!("../../tests/fixtures/file-page-captcha-recaptcha.html");
pub(crate) const CAPTCHA_CHECKBOX: &str =
    include_str!("../../tests/fixtures/file-page-captcha-checkbox.html");
pub(crate) const THRESHOLD: &str = include_str!("../../tests/fixtures/file-page-threshold.html");
pub(crate) const PASSWORD: &str = include_str!("../../tests/fixtures/file-page-password.html");
pub(crate) const MALWARE: &str = include_str!("../../tests/fixtures/file-page-malware.html");

pub(crate) const KEY: &str = "ipnyzofjcwri357";
pub(crate) const FILE_URL: &str =
    "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file";
pub(crate) const PAGE_URL: &str = "https://www.mediafire.com/file/ipnyzofjcwri357";
pub(crate) const API_URL: &str = "https://www.mediafire.com/api/1.5/file/get_info.php";
pub(crate) const DIRECT_URL: &str =
    "https://download2269.mediafire.com/redacted-token/ipnyzofjcwri357/test-10mb.bin";

pub(crate) struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    pub(crate) requests: Mutex<Vec<HostHttpRequest>>,
    pub(crate) captchas: Mutex<Vec<rd_plugin_api::CaptchaChallenge>>,
    /// Token every captcha is answered with; `None` mimics a host with no solver.
    captcha_token: Option<String>,
}

impl MockHost {
    pub(crate) fn with_responses(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Self::solving(responses, None)
    }

    pub(crate) fn solving(responses: Vec<HostHttpResponse>, token: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            captcha_token: token.map(str::to_owned),
        })
    }

    pub(crate) fn requests(&self) -> Vec<HostHttpRequest> {
        self.requests.lock().expect("mock lock").clone()
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
            .ok_or_else(|| Failure::new(rd_core::FailureKind::Permanent, "missing mock response"))
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
                rd_core::FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            )),
        }
    }
}

pub(crate) fn json(status: u16, body: &[u8]) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: API_URL.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: body.to_vec(),
    }
}

pub(crate) fn html_at(final_url: &str, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: final_url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

pub(crate) fn html(body: &str) -> HostHttpResponse {
    html_at(FILE_URL, body)
}

pub(crate) fn resolver(host: &Arc<MockHost>) -> MediafireResolver {
    MediafireResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

/// An account-less request, which is the only kind this provider serves.
pub(crate) fn resolve_request(url: &str) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

// --- URL recognition -----------------------------------------------------------------------

/// Every file form measured live on 2026-09-21 (job file, section 5), the JD-only forms, and
/// the `mfi.re` short host.
#[test]
fn every_measured_file_form_is_claimed() {
    let resolver = resolver(&MockHost::with_responses(Vec::new()));
    for url in [
        "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file",
        "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin",
        "https://www.mediafire.com/file/ipnyzofjcwri357",
        "https://mediafire.com/file/ipnyzofjcwri357",
        "https://www.mediafire.com/file_premium/ipnyzofjcwri357",
        "https://www.mediafire.com/download/ipnyzofjcwri357",
        "https://www.mediafire.com/view/ipnyzofjcwri357",
        "https://www.mediafire.com/?ipnyzofjcwri357",
        "https://www.mediafire.com/download.php?ipnyzofjcwri357",
        "https://mfi.re/?ipnyzofjcwri357",
        "https://app.mediafire.com/ipnyzofjcwri357",
        "https://app.mediafire.com/file/ipnyzofjcwri357",
    ] {
        assert!(resolver.matches(&url.parse::<Url>().expect("URL")), "{url}");
    }
}

/// Folders, key lists and everything else are somebody else's.
#[test]
fn folders_lists_and_foreign_addresses_are_not_claimed() {
    let resolver = resolver(&MockHost::with_responses(Vec::new()));
    for url in [
        "https://www.mediafire.com/folder/rww7bhhi0yc1l",
        "https://www.mediafire.com/folder/rww7bhhi0yc1l/shared",
        "https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx",
        "https://www.mediafire.com/",
        "https://www.mediafire.com/upgrade/get_plan.php",
        "https://download1514.mediafire.com/token/ipnyzofjcwri357/test-10mb.bin",
        "https://mediafire.com.evil.test/file/ipnyzofjcwri357",
        "https://example.com/file/ipnyzofjcwri357",
    ] {
        assert!(
            !resolver.matches(&url.parse::<Url>().expect("URL")),
            "{url}"
        );
    }
}

// --- the manifest ----------------------------------------------------------------------------

/// Every host the code contacts is granted, the measured delivery host is a download domain,
/// and nothing about a credential is declared, because there is none.
#[test]
fn the_manifest_grants_what_the_code_reaches_and_declares_no_secret() {
    let manifest: toml::Value = toml::from_str(crate::MANIFEST).expect("manifest parses");
    let list = |value: &toml::Value| -> Vec<String> {
        value
            .as_array()
            .expect("a list")
            .iter()
            .map(|item| item.as_str().expect("a string").to_owned())
            .collect()
    };
    let covers = |patterns: &[String], host: &str| {
        patterns
            .iter()
            .any(|pattern| match pattern.strip_prefix("*.") {
                Some(suffix) => host.ends_with(&format!(".{suffix}")),
                None => pattern == host,
            })
    };
    let net_http = list(&manifest["capabilities"]["net_http"]["domains"]);
    for host in [
        mediafire_common::address::PRIMARY_HOST,
        "download1514.mediafire.com",
        "download2269.mediafire.com",
    ] {
        assert!(covers(&net_http, host), "net_http must cover {host}");
    }
    let download = list(&manifest["download_domains"]);
    assert!(covers(&download, "download1514.mediafire.com"));
    assert!(covers(&download, "download1.mediafirecdn.com"));
    let matched = list(&manifest["match_domains"]);
    for host in mediafire_common::address::HOSTS {
        assert!(
            matched.contains(&(*host).to_owned()),
            "match_domains must list {host}"
        );
        assert!(covers(&net_http, host), "net_http must cover {host}");
    }
    assert!(manifest["capabilities"].get("secrets").is_none());
    assert!(manifest["provider"].get("secret_reference").is_none());
    assert_eq!(manifest["provider"]["credentials"].as_str(), Some("none"));
    assert_eq!(manifest["capabilities"]["cookies"].as_bool(), Some(false));
    assert_eq!(manifest["capabilities"]["captcha"].as_bool(), Some(true));
    assert!(manifest["limits"].get("wait_budget_milliseconds").is_none());
    assert_eq!(manifest["requires_account"].as_bool(), Some(false));
}

// --- the account-less contract ---------------------------------------------------------------

#[tokio::test]
async fn there_is_no_account_to_check_and_no_request_is_made() {
    let host = MockHost::with_responses(Vec::new());
    let failure = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect_err("no account");
    assert_eq!(failure.category, rd_core::FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("mediafire.no_account"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn hosters_are_the_two_domains() {
    let host = MockHost::with_responses(Vec::new());
    assert_eq!(
        resolver(&host)
            .hosters(AccountId::new())
            .await
            .expect("hosters"),
        vec!["mediafire.com".to_owned(), "mfi.re".to_owned()]
    );
}

#[path = "resolve_tests.rs"]
mod resolve_tests;

#[path = "captcha_tests.rs"]
mod captcha_tests;

#[path = "check_tests.rs"]
mod check_tests;

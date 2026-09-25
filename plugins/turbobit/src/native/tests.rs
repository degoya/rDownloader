//! The native adapter driven through the `Resolver` trait against the fixtures under
//! `tests/fixtures`, the way the scheduler drives it. The flow itself is tested in
//! `rd-plugin-turbobit-common`; what this covers is the conversion into the core's vocabulary
//! and the manifest this plugin ships.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CaptchaChallenge, CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse,
    ResolveRequest, ResolvedHeader, Resolver, ResolverHost,
};
use url::Url;

use super::TurbobitResolver;

macro_rules! fixture {
    ($file:literal) => {
        include_str!(concat!("../../tests/fixtures/", $file))
    };
}

pub(crate) struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    pub(crate) requests: Mutex<Vec<HostHttpRequest>>,
    pub(crate) waits: Mutex<Vec<u32>>,
    pub(crate) captchas: Mutex<Vec<CaptchaChallenge>>,
    has_password: bool,
}

impl MockHost {
    pub(crate) fn new(responses: Vec<HostHttpResponse>, has_password: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            has_password,
        })
    }
}

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.lock().expect("lock").push(request);
        self.responses
            .lock()
            .expect("lock")
            .pop_front()
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "missing mock response"))
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_password && reference == "turbobit_password"
    }

    async fn wait(&self, _client: &ClientIdentity, seconds: u32) -> Result<(), Failure> {
        self.waits.lock().expect("lock").push(seconds);
        Ok(())
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        challenge: CaptchaChallenge,
        _limit: std::time::Duration,
    ) -> Result<rd_plugin_api::CaptchaAnswer, Failure> {
        self.captchas.lock().expect("lock").push(challenge);
        Ok(rd_plugin_api::CaptchaAnswer::Token(
            "turnstile-token".to_owned(),
        ))
    }
}

pub(crate) fn json(status: u16, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://app.turbobit.net/api".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn html(status: u16, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://turbobit.net/".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn guest(url: &str) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

fn success() -> Vec<HostHttpResponse> {
    vec![
        json(200, fixture!("download-info-free-2026-09-21.json")),
        json(200, fixture!("free-init-ok-synthetic.json")),
        json(200, fixture!("captcha-2026-09-21.json")),
        json(200, fixture!("free-captcha-delay-synthetic.json")),
        json(200, fixture!("free-prepare-ok-synthetic.json")),
        json(200, fixture!("free-start-ok-synthetic.json")),
    ]
}

#[test]
fn every_live_domain_is_claimed_and_the_dead_and_foreign_ones_are_not() {
    let resolver = TurbobitResolver::new(MockHost::new(Vec::new(), false));
    for host in [
        "turbobit.net",
        "www.turbobit.net",
        "new.turbobit.net",
        "m.turbobit.net",
        "turbobit.cc",
        "turb.cc",
        "turb.pw",
        "turbo.to",
        "trbt.cc",
    ] {
        let url: Url = format!("https://{host}/a1b2c3d4e5f6.html")
            .parse()
            .expect("URL");
        assert!(resolver.matches(&url), "{host} should be claimed");
    }
    for url in [
        "https://turbobit.com/a1b2c3d4e5f6.html",
        "https://turbobit.online/a1b2c3d4e5f6.html",
        "https://hitfile.net/Ab1CdEf",
        "https://turbobit.net/rules",
    ] {
        assert!(!resolver.matches(&url.parse().expect("URL")), "{url}");
    }
}

/// Criterion 4: the manifest names the measured domains, the password's one host, the guest
/// concurrency and the wait budget.
#[test]
fn the_manifest_declares_the_measured_domains_limits_and_budget() {
    let manifest = crate::MANIFEST;
    for domain in [
        "\"turbobit.net\"",
        "\"www.turbobit.net\"",
        "\"new.turbobit.net\"",
        "\"m.turbobit.net\"",
        "\"turbobit.cc\"",
        "\"turb.cc\"",
        "\"turb.pw\"",
        "\"turbo.to\"",
        "\"trbt.cc\"",
    ] {
        assert!(
            manifest.contains(domain),
            "{domain} missing from match_domains"
        );
    }
    assert!(manifest.contains("download_domains = [\"turbobit.net\", \"*.turbobit.net\"]"));
    assert!(manifest.contains("domains = [\"turbobit.net\", \"app.turbobit.net\"]"));
    assert!(manifest.contains("secret_domains = [\"app.turbobit.net\"]"));
    assert!(manifest.contains("secrets = [\"turbobit_password\"]"));
    assert!(manifest.contains("captcha = true"));
    assert!(manifest.contains("cookies = false"));
    assert!(manifest.contains("wait_budget_milliseconds = 600000"));
    assert!(manifest.contains("max_response_bytes = 1048576"));
    let metadata = rd_plugin_api::metadata_from_manifest(manifest);
    assert_eq!(metadata.provider_slug, "turbobit");
    assert_eq!(metadata.max_concurrent_downloads, 1);
    assert!(!metadata.requires_account);
    // `domains` is what links are matched on; the request allowlist is checked above.
    assert_eq!(
        metadata.domains,
        vec![
            "turbobit.net",
            "www.turbobit.net",
            "new.turbobit.net",
            "m.turbobit.net",
            "turbobit.cc",
            "turb.cc",
            "turb.pw",
            "turbo.to",
            "trbt.cc"
        ]
    );
    // The version the manifest declares, not a copy of it: every release that raises the
    // plugin's version would otherwise have to edit this test too.
    let declared = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("the manifest declares a version");
    assert_eq!(metadata.version, declared);
}

#[tokio::test]
async fn a_guest_download_resolves_to_the_one_shot_link_without_fetching_it() {
    let host = MockHost::new(success(), false);
    let resolver = TurbobitResolver::new(host.clone());
    let resolved = resolver
        .resolve(guest(
            "https://turb.pw/a1b2c3d4e5f6.html?short_domain=turb.pw",
        ))
        .await
        .expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://turbobit.net/download/redirect/0123456789abcdef0123456789abcdef/a1b2c3d4e5f6/Sample%20File%201.pdf"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("Sample File 1.pdf"));
    assert_eq!(
        resolved.size.map(rd_core::ByteCount::get),
        Some(193_434_567)
    );
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://turbobit.net/download/started/a1b2c3d4e5f6")
    );
    let requests = host.requests.lock().expect("lock");
    assert_eq!(
        requests.len(),
        6,
        "info, init, captcha, captcha answer, prepare, start"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.url.host_str() == Some("app.turbobit.net")),
        "the short domain is never fetched"
    );
    assert_eq!(*host.waits.lock().expect("lock"), vec![60]);
    let captchas = host.captchas.lock().expect("lock");
    match &captchas[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, "0x4AAAAAACiD4nQO5axxHO3o");
        }
        other => panic!("expected Turnstile, got {other:?}"),
    }
}

#[tokio::test]
async fn a_deleted_file_is_offline_with_its_code() {
    let host = MockHost::new(
        vec![json(
            404,
            fixture!("download-info-deleted-404-2026-09-21.json"),
        )],
        false,
    );
    let failure = TurbobitResolver::new(host)
        .resolve(guest("https://turbobit.net/abcdefghijkl.html"))
        .await
        .expect_err("deleted");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("turbobit.file_unavailable"));
}

#[tokio::test]
async fn the_guest_window_is_an_ip_block_the_scheduler_holds_off_on() {
    let host = MockHost::new(
        vec![
            json(200, fixture!("download-info-free-2026-09-21.json")),
            json(200, fixture!("free-init-direct-hit-2026-09-21.json")),
        ],
        false,
    );
    let failure = TurbobitResolver::new(host.clone())
        .resolve(guest("https://turbobit.net/a1b2c3d4e5f6.html"))
        .await
        .expect_err("blocked");
    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("turbobit.free_limit_reached"));
    assert!(host.captchas.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_premium_only_file_is_refused_before_any_captcha() {
    let info = fixture!("download-info-free-2026-09-21.json").replace(
        "\"premiumOnlyDownload\":false",
        "\"premiumOnlyDownload\":true",
    );
    let host = MockHost::new(vec![json(200, &info)], false);
    let failure = TurbobitResolver::new(host.clone())
        .resolve(guest("https://turbobit.net/a1b2c3d4e5f6.html"))
        .await
        .expect_err("premium only");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("turbobit.premium_only"));
    assert_eq!(host.requests.lock().expect("lock").len(), 1);
}

/// Criterion 1: the site renders nothing server-side, so every page is the same 2.6 KiB shell;
/// it must never be what the scheduler downloads.
#[tokio::test]
async fn the_spa_shell_never_becomes_a_file() {
    let shell = fixture!("spa-shell-2026-09-21.html");
    for position in 0..6 {
        let mut answers = success();
        answers[position] = html(200, shell);
        let failure = TurbobitResolver::new(MockHost::new(answers, false))
            .resolve(guest("https://turbobit.net/a1b2c3d4e5f6.html"))
            .await
            .expect_err("a shell is never a result");
        assert_eq!(
            failure.code.as_deref(),
            Some("turbobit.invalid_response"),
            "{position}"
        );
        assert_eq!(failure.category, FailureKind::Permanent);
    }
    let failure = TurbobitResolver::new(MockHost::new(Vec::new(), false))
        .resolve(guest("https://turbobit.net/download/folder/123"))
        .await
        .expect_err("a folder");
    assert_eq!(failure.code.as_deref(), Some("turbobit.folder_not_file"));
}

#[tokio::test]
async fn check_reports_the_measured_statuses() {
    let host = MockHost::new(
        vec![json(200, fixture!("links-check-2026-09-21.json"))],
        true,
    );
    let results = TurbobitResolver::new(host)
        .check(CheckRequest {
            urls: vec![
                "https://turbobit.net/a1b2c3d4e5f6.html"
                    .parse()
                    .expect("URL"),
                "https://turbobit.net/abcdefghijkl.html"
                    .parse()
                    .expect("URL"),
                "https://turbobit.net/download/free/a1b2c3d4e5f6"
                    .parse()
                    .expect("URL"),
                "https://turb.pw/a1b2c3d4e5f6.html".parse().expect("URL"),
            ],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("checked");
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].status, rd_core::LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("Sample File 1.pdf"));
    assert_eq!(results[1].status, rd_core::LinkStatus::Offline);
    assert_eq!(results[2].status, rd_core::LinkStatus::Unknown);
    assert_eq!(results[3].status, rd_core::LinkStatus::Online);
}

#[tokio::test]
async fn check_account_signs_in_and_reports_the_subscription_in_bytes() {
    let host = MockHost::new(
        vec![
            json(
                401,
                fixture!("user-info-unauthenticated-401-2026-09-21.json"),
            ),
            json(200, fixture!("auth-login-ok-synthetic.json")),
            json(200, fixture!("user-info-premium-active-synthetic.json")),
            json(200, fixture!("premium-info-synthetic.json")),
        ],
        true,
    );
    let status = TurbobitResolver::new(host.clone())
        .check_account(AccountId::new())
        .await
        .expect("checked");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        status.traffic_left.map(rd_core::ByteCount::get),
        Some(12 * 1024 * 1024 * 1024 + 512 * 1024 * 1024)
    );
    let label = plugin_common::native::label_summary(&status.label);
    assert!(
        label.contains("plugin.account.user(user=user@example.test)"),
        "{label}"
    );
    assert!(
        label.contains("plugin.account.premium_until(until=2099-01-01 00:00:00)"),
        "{label}"
    );
    let login = &host.requests.lock().expect("lock")[1];
    assert_eq!(
        login.url.as_str(),
        "https://app.turbobit.net/api/auth/login"
    );
    let body = String::from_utf8_lossy(&login.body);
    assert!(body.contains("{{secret:turbobit_password}}"), "{body}");
    assert!(body.contains("{{username}}"), "{body}");
}

#[tokio::test]
async fn without_a_password_the_account_is_refused_before_any_request() {
    let host = MockHost::new(Vec::new(), false);
    let failure = TurbobitResolver::new(host.clone())
        .check_account(AccountId::new())
        .await
        .expect_err("no password");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("turbobit.account_missing"));
    assert!(host.requests.lock().expect("lock").is_empty());
}

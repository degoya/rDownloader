//! The native adapter driven through the `Resolver` trait against the fixtures under
//! `tests/fixtures`. The flow itself is tested in `rd-plugin-turbobit-common`; what this
//! covers is HitFile's own parameters — the id shapes, the `.html`-less canonical link, the
//! live premium-only file — and the manifest this plugin ships.

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

use super::HitfileResolver;

macro_rules! fixture {
    ($file:literal) => {
        include_str!(concat!("../../tests/fixtures/", $file))
    };
}

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    waits: Mutex<Vec<u32>>,
    captchas: Mutex<Vec<CaptchaChallenge>>,
}

impl MockHost {
    fn new(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            waits: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
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

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
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

fn json(status: u16, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://app.hitfile.net/api".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
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

#[test]
fn every_live_domain_is_claimed_in_both_id_shapes() {
    let resolver = HitfileResolver::new(MockHost::new(Vec::new()));
    for host in [
        "hitfile.net",
        "www.hitfile.net",
        "new.hitfile.net",
        "hitfile.ru",
        "hil.to",
        "hitf.cc",
        "htfl.net",
        "hitf.to",
    ] {
        for path in ["/Ab1CdEf", "/0ZGT", "/Ab1CdEf/premium-only-sample.rar.html"] {
            let url: Url = format!("https://{host}{path}").parse().expect("URL");
            assert!(resolver.matches(&url), "{url} should be claimed");
        }
    }
    for url in [
        "https://hitfile.to/Ab1CdEf",
        "https://hitfile.net/premium",
        "https://hitfile.net/login",
        "https://hitfile.net/abcdefgh",
        "https://turbobit.net/a1b2c3d4e5f6.html",
    ] {
        assert!(!resolver.matches(&url.parse().expect("URL")), "{url}");
    }
}

/// Criterion 4, for this brand's own lists.
#[test]
fn the_manifest_declares_the_measured_domains_limits_and_budget() {
    let manifest = crate::MANIFEST;
    for domain in [
        "\"hitfile.net\"",
        "\"www.hitfile.net\"",
        "\"new.hitfile.net\"",
        "\"hitfile.ru\"",
        "\"hil.to\"",
        "\"hitf.cc\"",
        "\"htfl.net\"",
        "\"hitf.to\"",
    ] {
        assert!(
            manifest.contains(domain),
            "{domain} missing from match_domains"
        );
    }
    assert!(manifest.contains("download_domains = [\"hitfile.net\", \"*.hitfile.net\"]"));
    assert!(manifest.contains("domains = [\"hitfile.net\", \"app.hitfile.net\"]"));
    assert!(manifest.contains("secret_domains = [\"app.hitfile.net\"]"));
    assert!(manifest.contains("secrets = [\"hitfile_password\"]"));
    assert!(manifest.contains("wait_budget_milliseconds = 600000"));
    let metadata = rd_plugin_api::metadata_from_manifest(manifest);
    assert_eq!(metadata.provider_slug, "hitfile");
    assert_eq!(metadata.max_concurrent_downloads, 1);
    assert!(!metadata.requires_account);
    // `domains` is what links are matched on; the request allowlist is checked above.
    assert_eq!(
        metadata.domains,
        vec![
            "hitfile.net",
            "www.hitfile.net",
            "new.hitfile.net",
            "hitfile.ru",
            "hil.to",
            "hitf.cc",
            "htfl.net",
            "hitf.to"
        ]
    );
}

#[tokio::test]
async fn a_guest_download_resolves_with_hitfiles_own_key_and_countdown() {
    let host = MockHost::new(vec![
        json(200, fixture!("download-info-free-2026-09-21.json")),
        json(200, fixture!("free-init-ok-synthetic.json")),
        json(200, fixture!("captcha-2026-09-21.json")),
        json(200, fixture!("free-captcha-delay-synthetic.json")),
        json(200, fixture!("free-prepare-ok-synthetic.json")),
        json(200, fixture!("free-start-ok-synthetic.json")),
    ]);
    let resolved = HitfileResolver::new(host.clone())
        .resolve(guest("https://hitfile.ru/Gh2IjKl"))
        .await
        .expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://hitfile.net/download/redirect/0123456789abcdef0123456789abcdef/Gh2IjKl/free-sample.zip"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("free-sample.zip"));
    assert_eq!(
        resolved.size.map(rd_core::ByteCount::get),
        Some(946_055_308)
    );
    let requests = host.requests.lock().expect("lock");
    assert_eq!(requests.len(), 6);
    assert!(
        requests
            .iter()
            .all(|request| request.url.host_str() == Some("app.hitfile.net")),
        "hitfile.ru is never fetched"
    );
    assert_eq!(*host.waits.lock().expect("lock"), vec![38]);
    match &host.captchas.lock().expect("lock")[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, "0x4AAAAAACiD-ljaJGPKwX-V");
            assert_eq!(widget.page_url, "https://hitfile.net/download/free/Gh2IjKl");
        }
        other => panic!("expected Turnstile, got {other:?}"),
    }
}

/// The public premium-only file, measured live: refused on `download/info`, before any
/// captcha is spent (criterion 2).
#[tokio::test]
async fn the_live_premium_only_file_is_refused_before_the_captcha() {
    let host = MockHost::new(vec![json(
        200,
        fixture!("download-info-premium-only-2026-09-21.json"),
    )]);
    let failure = HitfileResolver::new(host.clone())
        .resolve(guest("https://hitfile.net/Ab1CdEf"))
        .await
        .expect_err("premium only");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("hitfile.premium_only"));
    assert_eq!(host.requests.lock().expect("lock").len(), 1);
    assert!(host.captchas.lock().expect("lock").is_empty());
}

/// The same file's later answers, measured live, each end in one code — should a future
/// `download/info` stop saying `premiumOnlyDownload`.
#[tokio::test]
async fn the_measured_start_refusals_end_in_structured_codes() {
    let free_info = fixture!("download-info-premium-only-2026-09-21.json").replace(
        "\"premiumOnlyDownload\":true",
        "\"premiumOnlyDownload\":false",
    );
    for (answer, code, category) in [
        (
            json(409, fixture!("free-start-premium-only-409-2026-09-21.json")),
            "hitfile.no_direct_link",
            FailureKind::Permanent,
        ),
        (
            json(
                400,
                fixture!("free-start-premium-only-400-feasibility.json"),
            ),
            "hitfile.premium_only",
            FailureKind::AuthRequired,
        ),
        (
            json(404, fixture!("free-start-not-found-404-2026-09-21.json")),
            "hitfile.file_unavailable",
            FailureKind::Offline,
        ),
    ] {
        let host = MockHost::new(vec![
            json(200, &free_info),
            json(200, fixture!("free-init-ok-synthetic.json")),
            json(200, fixture!("captcha-2026-09-21.json")),
            json(200, fixture!("free-captcha-delay-synthetic.json")),
            json(200, fixture!("free-prepare-ok-synthetic.json")),
            answer,
        ]);
        let failure = HitfileResolver::new(host)
            .resolve(guest("https://hitfile.net/Ab1CdEf"))
            .await
            .expect_err(code);
        assert_eq!(failure.code.as_deref(), Some(code));
        assert_eq!(failure.category, category, "{code}");
    }
}

#[tokio::test]
async fn direct_hit_on_the_premium_only_file_is_an_ip_block() {
    let free_info = fixture!("download-info-premium-only-2026-09-21.json").replace(
        "\"premiumOnlyDownload\":true",
        "\"premiumOnlyDownload\":false",
    );
    let host = MockHost::new(vec![
        json(200, &free_info),
        json(
            200,
            fixture!("free-init-direct-hit-premium-only-2026-09-21.json"),
        ),
    ]);
    let failure = HitfileResolver::new(host)
        .resolve(guest("https://hitfile.net/Ab1CdEf"))
        .await
        .expect_err("blocked");
    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("hitfile.free_limit_reached"));
}

#[tokio::test]
async fn a_deleted_file_is_offline() {
    let host = MockHost::new(vec![json(
        404,
        fixture!("download-info-deleted-404-2026-09-21.json"),
    )]);
    let failure = HitfileResolver::new(host)
        .resolve(guest("https://hitfile.net/Mn3OpQr"))
        .await
        .expect_err("deleted");
    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("hitfile.file_unavailable"));
}

#[tokio::test]
async fn check_sends_the_html_less_canonical_link_and_maps_the_measured_answer() {
    let host = MockHost::new(vec![json(
        200,
        fixture!("links-check-mixed-2026-09-21.json"),
    )]);
    let results = HitfileResolver::new(host.clone())
        .check(CheckRequest {
            urls: vec![
                "https://hitfile.net/Ab1CdEf".parse().expect("URL"),
                "https://hitfile.net/Gh2IjKl".parse().expect("URL"),
                "https://hitfile.net/Mn3OpQr".parse().expect("URL"),
                "https://hitfile.net/Gh2IjKl.html".parse().expect("URL"),
                "https://hil.to/Gh2IjKl".parse().expect("URL"),
            ],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("checked");
    assert_eq!(results.len(), 5);
    assert_eq!(results[0].status, rd_core::LinkStatus::Online);
    assert_eq!(
        results[0].file_name.as_deref(),
        Some("premium-only-sample.rar")
    );
    assert_eq!(results[2].status, rd_core::LinkStatus::Offline);
    assert_eq!(results[4].status, rd_core::LinkStatus::Online);
    let body = String::from_utf8_lossy(&host.requests.lock().expect("lock")[0].body).into_owned();
    assert!(!body.contains(".html"), "{body}");
    assert!(!body.contains("hil.to"), "{body}");
}

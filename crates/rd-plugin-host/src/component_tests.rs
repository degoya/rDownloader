use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver, ResolverHost,
};
use url::Url;

use super::{
    ComponentResolver, MAX_RANDOM_BYTES, from_wit_download, random_bytes, wit_captcha, wit_http,
    wit_types,
};
use crate::{PluginLimits, PluginManifest, SandboxEngine, domain_allowed};

struct MockHost {
    calls: AtomicUsize,
}

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(HostHttpResponse {
            status: 200,
            final_url: request.url,
            headers: Vec::new(),
            body: b"ok".to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn manifest_domain_is_checked_before_host_http() {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let host = Arc::new(MockHost {
        calls: AtomicUsize::new(0),
    });
    let identity = ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    };
    let mut store = sandbox
        .create_invocation_store(vec!["api.example.test".to_owned()], host.clone(), identity)
        .expect("store");

    let result = wit_http::Host::http_request(
        store.data_mut(),
        "GET".to_owned(),
        "https://attacker.invalid/".to_owned(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
}

/// Answers every captcha instantly and remembers how long it was allowed to take.
struct CaptchaHost {
    allowance: std::time::Duration,
    granted: std::sync::Mutex<Option<std::time::Duration>>,
}

impl CaptchaHost {
    fn new(allowance: std::time::Duration) -> Arc<Self> {
        Arc::new(Self {
            allowance,
            granted: std::sync::Mutex::new(None),
        })
    }

    fn granted(&self) -> Option<std::time::Duration> {
        *self.granted.lock().expect("mock lock")
    }
}

#[async_trait]
impl ResolverHost for CaptchaHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        _request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        Err(Failure::new(
            rd_core::FailureKind::Unsupported,
            "no HTTP in this test",
        ))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }

    async fn captcha_allowance(&self) -> std::time::Duration {
        self.allowance
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        challenge: rd_plugin_api::CaptchaChallenge,
        time_limit: std::time::Duration,
    ) -> Result<rd_plugin_api::CaptchaAnswer, Failure> {
        *self.granted.lock().expect("mock lock") = Some(time_limit);
        Ok(if challenge.answers_with_point() {
            rd_plugin_api::CaptchaAnswer::Point(rd_plugin_api::ClickPoint { x: 120, y: 44 })
        } else {
            rd_plugin_api::CaptchaAnswer::Token("answer".to_owned())
        })
    }
}

fn captcha_store(
    host: Arc<CaptchaHost>,
    wait_budget_milliseconds: u64,
) -> wasmtime::Store<crate::runtime::PluginStoreState> {
    let sandbox = SandboxEngine::new(PluginLimits {
        wait_budget_milliseconds,
        ..PluginLimits::default()
    })
    .expect("sandbox");
    sandbox
        .create_invocation_store(
            vec!["example.test".to_owned()],
            host,
            ClientIdentity {
                account_id: None,
                proxy_profile_id: None,
                tls_revision: 0,
            },
        )
        .expect("store")
}

fn image_challenge(bytes: usize) -> wit_captcha::CaptchaChallenge {
    wit_captcha::CaptchaChallenge::Image(wit_captcha::ImageChallenge {
        mime: "image/png".to_owned(),
        data: vec![0; bytes],
        prompt: None,
    })
}

fn click_point_challenge() -> wit_captcha::CaptchaChallenge {
    wit_captcha::CaptchaChallenge::ClickPoint(wit_captcha::ImageChallenge {
        mime: "image/png".to_owned(),
        data: vec![0; 64],
        prompt: Some("Click the circle".to_owned()),
    })
}

fn cutcaptcha_challenge(misery_key: &str) -> wit_captcha::CaptchaChallenge {
    wit_captcha::CaptchaChallenge::Cutcaptcha(wit_captcha::CutcaptchaChallenge {
        site_key: "SAs61IAI".to_owned(),
        misery_key: misery_key.to_owned(),
        page_url: "https://example.test/Container/ABC.html".to_owned(),
    })
}

/// RD-110-15: the click a person made reaches the plugin as a coordinate, through
/// `solve-challenge`, and a token-shaped kind still comes back as a token from the same call.
#[tokio::test]
async fn a_click_point_challenge_is_answered_with_a_point_through_solve_challenge() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(300));
    let mut store = captcha_store(host.clone(), 600_000);

    let answer = wit_captcha::Host::solve_challenge(store.data_mut(), click_point_challenge())
        .await
        .expect("answered");
    assert!(
        matches!(
            answer,
            wit_captcha::CaptchaAnswer::Point(wit_captcha::ClickPoint { x: 120, y: 44 })
        ),
        "{answer:?}"
    );

    let token =
        wit_captcha::Host::solve_challenge(store.data_mut(), cutcaptcha_challenge("misery"))
            .await
            .expect("answered");
    assert!(
        matches!(token, wit_captcha::CaptchaAnswer::Token(ref value) if value == "answer"),
        "{token:?}"
    );
}

/// `solve-captcha` cannot carry a point, so a click-point challenge is refused there before
/// anybody is asked and before the plugin pays any waiting time for it.
#[tokio::test]
async fn solve_captcha_refuses_a_click_point_before_asking_anyone() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(300));
    let mut store = captcha_store(host.clone(), 600_000);

    let refused = wit_captcha::Host::solve_captcha(store.data_mut(), click_point_challenge())
        .await
        .expect_err("the token form cannot answer a click");
    assert_eq!(refused.code.as_deref(), Some("captcha.answer_shape"));
    assert_eq!(host.granted(), None, "nobody was asked");
    assert_eq!(
        store.data().wait_budget(),
        std::time::Duration::from_secs(600)
    );
}

/// A CutCaptcha is held to the same boundary as every other widget: both keys must be usable
/// and the page must lie inside the plugin's declared domains.
#[tokio::test]
async fn a_cutcaptcha_needs_both_keys_and_a_page_inside_the_plugins_domains() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(300));
    let mut store = captcha_store(host.clone(), 600_000);

    let no_key = wit_captcha::Host::solve_challenge(store.data_mut(), cutcaptcha_challenge(""))
        .await
        .expect_err("an empty misery key");
    assert_eq!(no_key.code.as_deref(), Some("captcha.site_key_invalid"));

    let elsewhere = wit_captcha::Host::solve_challenge(
        store.data_mut(),
        wit_captcha::CaptchaChallenge::Cutcaptcha(wit_captcha::CutcaptchaChallenge {
            site_key: "SAs61IAI".to_owned(),
            misery_key: "misery".to_owned(),
            page_url: "https://attacker.invalid/".to_owned(),
        }),
    )
    .await
    .expect_err("a page outside the manifest");
    assert_eq!(
        elsewhere.code.as_deref(),
        Some("plugin.captcha_target_not_allowed")
    );
    assert_eq!(host.granted(), None);
}

/// Solving runs on a service or a person, so the plugin's waiting budget has to cover it —
/// and a generous manual timeout needs more than a fixed allowance would ever grant.
#[tokio::test]
async fn a_captcha_reserves_the_time_the_host_says_answering_needs() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(300));
    let mut store = captcha_store(host.clone(), 600_000);

    let solution = wit_captcha::Host::solve_captcha(store.data_mut(), image_challenge(64))
        .await
        .expect("solved");

    assert_eq!(solution.token, "answer");
    assert_eq!(
        host.granted(),
        Some(std::time::Duration::from_secs(300)),
        "the host must be told how long it may take"
    );
    assert_eq!(
        store.data().wait_budget(),
        std::time::Duration::from_secs(300),
        "the reservation comes out of the same budget as hoster countdowns"
    );
}

/// A countdown that already ate the budget leaves less than the host asked for. Answering
/// still goes ahead, but only within what is actually left.
#[tokio::test]
async fn a_captcha_settles_for_the_budget_that_is_left() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(600));
    let mut store = captcha_store(host.clone(), 90_000);

    wit_captcha::Host::solve_captcha(store.data_mut(), image_challenge(64))
        .await
        .expect("solved");

    assert_eq!(host.granted(), Some(std::time::Duration::from_secs(90)));
    assert_eq!(store.data().wait_budget(), std::time::Duration::ZERO);
}

#[tokio::test]
async fn a_captcha_is_refused_when_no_useful_time_is_left() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(300));
    let mut store = captcha_store(host.clone(), 5_000);

    let failure = wit_captcha::Host::solve_captcha(store.data_mut(), image_challenge(64))
        .await
        .expect_err("no budget left");

    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.wait_budget_exhausted")
    );
    assert_eq!(host.granted(), None, "nobody was asked to solve anything");
}

/// A challenge no service could work with must be rejected before it costs waiting time or
/// reaches a paid solver.
#[tokio::test]
async fn an_unusable_challenge_is_refused_before_any_budget_is_spent() {
    let host = CaptchaHost::new(std::time::Duration::from_secs(300));
    let mut store = captcha_store(host.clone(), 600_000);

    let oversized = wit_captcha::Host::solve_captcha(store.data_mut(), image_challenge(600 * 1024))
        .await
        .expect_err("image too large");
    assert_eq!(oversized.code.as_deref(), Some("captcha.image_invalid"));

    let empty_key = wit_captcha::Host::solve_captcha(
        store.data_mut(),
        wit_captcha::CaptchaChallenge::RecaptchaV2(wit_captcha::WidgetChallenge {
            site_key: String::new(),
            page_url: "https://example.test/file".to_owned(),
            invisible: false,
        }),
    )
    .await
    .expect_err("unusable site key");
    assert_eq!(empty_key.code.as_deref(), Some("captcha.site_key_invalid"));

    assert_eq!(
        store.data().wait_budget(),
        std::time::Duration::from_secs(600),
        "a rejected challenge must not cost the plugin any waiting time"
    );
    assert_eq!(host.granted(), None);
}

#[test]
fn component_cannot_replace_bound_client_identity() {
    let expected = ClientIdentity {
        account_id: Some(AccountId::new()),
        proxy_profile_id: None,
        tls_revision: 7,
    };
    let value = wit_types::ResolvedDownload {
        url: "https://downloads.example.test/file".to_owned(),
        file_name: None,
        size: Some(2),
        headers: Vec::new(),
        checksum_algorithm: None,
        checksum_value: None,
        client: wit_types::ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 7,
        },
    };

    let result = from_wit_download(value, expected, &["downloads.example.test".to_owned()]);

    assert!(result.is_err());
}

#[test]
fn component_without_required_resolver_exports_is_rejected_at_load() {
    let manifest: PluginManifest = toml::from_str(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000000fe"
name = "Broken"
version = "1.0.0"
key_id = "fixture"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "Broken fixture"
author = "Fixture Author"

[provider]
slug = "broken"
kind = "hoster"
credentials = "api_key"
"#,
    )
    .expect("manifest");
    let host: Arc<dyn ResolverHost> = Arc::new(MockHost {
        calls: AtomicUsize::new(0),
    });

    let result = ComponentResolver::new(manifest, b"\0asm\x0d\0\x01\0", host);

    assert!(result.is_err());
}

#[test]
fn ddownload_manifest_allows_its_https_cdn_without_matching_input_links() {
    let manifest: PluginManifest = toml::from_str(
        &std::fs::read_to_string("../../plugins/ddownload/manifest.toml").expect("manifest"),
    )
    .expect("parse manifest");
    let cdn = Url::parse("https://eu-hydra5.zeuscdn.org:183/d/token/release.rar").expect("url");

    assert!(domain_allowed(&cdn, manifest.domains()));
    assert!(domain_allowed(&cdn, &manifest.download_domains));
    assert!(!domain_allowed(&cdn, &manifest.match_domains));
}

/// Runs the real DDownload component against a large synthetic file page. Guards
/// the fuel budget: parsing two ~200 KiB HTML documents must not exhaust it.
#[tokio::test]
async fn real_ddownload_component_parses_a_large_page_within_its_fuel_budget() {
    let component = crate::artifact::component("rd-plugin-ddownload");
    let mut page = String::from("<html><head><title>DDownload</title></head><body>");
    for index in 0..1_500 {
        page.push_str(&format!(
            "<div class=\"row\"><a href=\"https://ddownload.com/page{index}\">Link {index}</a> \
             <span>Lorem ipsum dolor sit amet, consectetur adipiscing elit</span></div>\n"
        ));
    }
    page.push_str(
        "<form method=\"POST\" action=\"\"><input type=\"hidden\" name=\"op\" value=\"download2\">\
         <input type=\"hidden\" name=\"id\" value=\"z31889n8peey\">\
         <input type=\"hidden\" name=\"rand\" value=\"abc\"></form></body></html>",
    );
    assert!(page.len() > 150 * 1024, "fixture should be large");

    struct PageHost(Vec<u8>, std::sync::Mutex<usize>);
    #[async_trait]
    impl ResolverHost for PageHost {
        async fn http_request(
            &self,
            _client: &ClientIdentity,
            request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            *self.1.lock().expect("lock") += 1;
            Ok(HostHttpResponse {
                status: 200,
                final_url: request.url,
                headers: vec![rd_plugin_api::ResolvedHeader {
                    name: "content-type".to_owned(),
                    value: "text/html; charset=UTF-8".to_owned(),
                }],
                body: self.0.clone(),
            })
        }
        async fn cookies_get(
            &self,
            _account_id: AccountId,
            _url: &url::Url,
        ) -> Vec<(String, String)> {
            vec![("xfss".to_owned(), "abc".to_owned())]
        }
        async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
            false
        }
    }
    let manifest: PluginManifest = toml::from_str(
        &std::fs::read_to_string("../../plugins/ddownload/manifest.toml").expect("manifest"),
    )
    .expect("parse manifest");
    let host = Arc::new(PageHost(page.into_bytes(), std::sync::Mutex::new(0)));
    let resolver = ComponentResolver::new(manifest, &component, host.clone()).expect("load");
    let request = ResolveRequest {
        url: Url::parse("https://ddownload.com/z31889n8peey/Idle_Immortal.rar").expect("url"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    };

    let error = resolver
        .resolve(request)
        .await
        .expect_err("no premium file");

    assert_eq!(*host.1.lock().expect("lock"), 2, "GET page, POST form");
    assert!(
        !error.message.starts_with("Plugin execution failed"),
        "component trapped: {}",
        error.message
    );
    assert!(
        error.message.contains("did not return a premium file"),
        "{}",
        error.message
    );
}

/// The entropy behind every PKCE verifier and OAuth `state` a guest builds. It has to be
/// exactly as long as asked for, and different every time — a repeat or a truncated answer is
/// the failure mode a guest would not notice.
///
/// Distinctness is only asserted from 16 bytes upwards, where a coincidence has probability
/// 2^-128; asserting it on a single byte would be a test that fails once in 256 runs.
#[test]
fn random_bytes_are_the_requested_length_and_never_repeat() {
    for count in [1u32, 16, 32, MAX_RANDOM_BYTES] {
        let bytes = random_bytes(count);
        assert_eq!(bytes.len(), count as usize);
        if count >= 16 {
            assert_ne!(
                bytes,
                random_bytes(count),
                "two draws of {count} bytes were identical"
            );
            assert!(
                bytes.iter().any(|byte| *byte != 0),
                "{count} bytes came back all zero"
            );
        }
    }
}

#[test]
fn random_bytes_refuses_nothing_and_too_much() {
    assert!(random_bytes(0).is_empty());
    assert!(random_bytes(MAX_RANDOM_BYTES + 1).is_empty());
    assert!(random_bytes(u32::MAX).is_empty());
}

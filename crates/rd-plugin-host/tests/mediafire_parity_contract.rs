//! The MediaFire resolver's native fallback and its WebAssembly component pass the same
//! contract (RD-103-06, criterion 3).
//!
//! The same scripted host answers both: the captured `file/get_info` document and file page
//! of 2026-09-21 for the success path, the captured refusals and the synthetic page states
//! for the failures. What is asserted is not that each side behaves — the plugin crate's own
//! tests do that natively — but that the two sides behave *identically*: the same URL
//! recognition, the same resolved address, name, size and checksum, the same failure code,
//! category and parameters, and the same requests to the host in the same order.
//!
//! The component is loaded by `rd_plugin_host::artifact::component`, which fails rather than
//! passes when it is missing or older than its sources (RD-108-16).

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};
use rd_plugin_host::{ComponentResolver, PluginManifest};

const FILE_PAGE: &str =
    include_str!("../../../plugins/mediafire/tests/fixtures/file-page-2026-09-21.html");
const GET_INFO: &str =
    include_str!("../../../plugins/mediafire/tests/fixtures/api-file-get-info-2026-09-21.json");
const GET_INFO_BATCH: &str = include_str!(
    "../../../plugins/mediafire/tests/fixtures/api-file-get-info-batch-2026-09-21.json"
);
const GET_INFO_INVALID: &str = include_str!(
    "../../../plugins/mediafire/tests/fixtures/api-file-get-info-invalid-2026-09-21.json"
);
const API_ERROR_261: &str =
    include_str!("../../../plugins/mediafire/tests/fixtures/api-error-261.json");
const THRESHOLD: &str =
    include_str!("../../../plugins/mediafire/tests/fixtures/file-page-threshold.html");
const PASSWORD: &str =
    include_str!("../../../plugins/mediafire/tests/fixtures/file-page-password.html");

const FILE_URL: &str = "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file";

fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-mediafire")
}

/// A scripted host: answers in order, records every request as `METHOD url?query`.
struct ScriptedHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<String>>,
}

impl ScriptedHost {
    fn new(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ResolverHost for ScriptedHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let mut query: Vec<String> = request
            .query
            .iter()
            .map(|item| format!("{}={}", item.name, item.value_template))
            .collect();
        query.sort();
        let mut headers: Vec<String> = request
            .headers
            .iter()
            .map(|item| format!("{}: {}", item.name, item.value_template))
            .collect();
        headers.sort();
        self.requests.lock().expect("requests").push(format!(
            "{} {} [{}] {{{}}} {}",
            request.method,
            request.url,
            query.join("&"),
            headers.join(", "),
            String::from_utf8_lossy(&request.body)
        ));
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or_else(|| Failure::new(rd_core::FailureKind::Permanent, "missing response"))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

fn json(status: u16, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://www.mediafire.com/api/1.5/file/get_info.php"
            .parse()
            .expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn html(final_url: &str, body: &str) -> HostHttpResponse {
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

fn request(url: &str) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

/// Both sides of the plugin, each on its own copy of the same script.
fn both(script: &[HostHttpResponse]) -> [(Arc<ScriptedHost>, Box<dyn Resolver>); 2] {
    let native_host = ScriptedHost::new(script.to_vec());
    let native: Box<dyn Resolver> = Box::new(rd_plugin_mediafire::MediafireResolver::new(
        Arc::clone(&native_host) as Arc<dyn ResolverHost>,
    ));
    let component_host = ScriptedHost::new(script.to_vec());
    let manifest: PluginManifest =
        toml::from_str(rd_plugin_mediafire::MANIFEST).expect("the manifest");
    let component: Box<dyn Resolver> = Box::new(
        ComponentResolver::new(
            manifest,
            &component(),
            Arc::clone(&component_host) as Arc<dyn ResolverHost>,
        )
        .expect("the component instantiates"),
    );
    [(native_host, native), (component_host, component)]
}

/// A resolve outcome flattened to what the scheduler acts on, comparable across both sides.
async fn resolve_outcome(resolver: &dyn Resolver, url: &str) -> String {
    match resolver.resolve(request(url)).await {
        Ok(resolved) => format!(
            "ok url={} name={:?} size={:?} checksum={:?} headers={}",
            resolved.url,
            resolved.file_name,
            resolved.size.map(rd_core::ByteCount::get),
            resolved
                .checksum
                .map(|checksum| format!("{:?}:{}", checksum.algorithm, checksum.value)),
            resolved.headers.len()
        ),
        Err(failure) => format!(
            "err code={:?} category={:?} params={:?} message={}",
            failure.code, failure.category, failure.params, failure.message
        ),
    }
}

/// The guest's `match-url` is the same function the native `matches` runs, but the host
/// reaches it differently: `ComponentResolver::matches` answers from the manifest's domains
/// alone (the sandbox is not entered for a dispatch question) and asks the guest at the
/// start of `resolve`, refusing with `plugin.url_rejected` when it says no. So the plugin's
/// decision is compared where both sides expose it: the native `matches` against whether the
/// component's `resolve` got past the guest's answer.
#[tokio::test]
async fn both_sides_recognise_the_same_addresses() {
    for (url, claimed) in [
        (
            "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file",
            true,
        ),
        ("https://www.mediafire.com/download/ipnyzofjcwri357", true),
        ("https://www.mediafire.com/view/ipnyzofjcwri357", true),
        ("https://www.mediafire.com/?ipnyzofjcwri357", true),
        (
            "https://www.mediafire.com/download.php?ipnyzofjcwri357",
            true,
        ),
        ("https://mfi.re/?ipnyzofjcwri357", true),
        ("https://app.mediafire.com/ipnyzofjcwri357", true),
        ("https://www.mediafire.com/folder/rww7bhhi0yc1l", false),
        (
            "https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx",
            false,
        ),
        ("https://www.mediafire.com/", false),
        ("https://example.com/file/ipnyzofjcwri357", false),
    ] {
        let [(_, native), (_, component)] = both(&[]);
        let parsed = url.parse().expect("URL");
        assert_eq!(native.matches(&parsed), claimed, "{url}: native");
        // Refused by the guest (`url_rejected`) or by the domain gate in front of it
        // (`url_outside_domains`): either way the plugin never resolves it.
        let outcome = resolve_outcome(component.as_ref(), url).await;
        let component_claims = !outcome.contains("plugin.url_rejected")
            && !outcome.contains("plugin.url_outside_domains");
        assert_eq!(component_claims, claimed, "{url}: component");
        // What the host dispatches on: the manifest's domains, the same file for both.
        assert_eq!(
            component.matches(&parsed),
            parsed
                .host_str()
                .is_some_and(|host| host.ends_with("mediafire.com") || host == "mfi.re"),
            "{url}: domain dispatch"
        );
    }
    let [(_, native), (_, component)] = both(&[]);
    assert_eq!(native.metadata().provider_slug, "mediafire");
    assert_eq!(component.metadata().provider_slug, "mediafire");
    assert_eq!(native.metadata().version, component.metadata().version);
}

#[tokio::test]
async fn both_sides_resolve_the_captured_file_to_the_same_download() {
    let script = [json(200, GET_INFO), html(FILE_URL, FILE_PAGE)];
    let [(native_host, native), (component_host, component)] = both(&script);
    let expected = resolve_outcome(native.as_ref(), FILE_URL).await;
    assert!(
        expected.starts_with("ok url=https://download2269.mediafire.com/"),
        "{expected}"
    );
    assert!(expected.contains("name=Some(\"test-10mb.bin\") size=Some(10485760)"));
    assert!(
        expected
            .contains("Sha256:e5b844cc57f57094ea4585e235f36c78c1cd222262bb89d53c94dcb4d6b3e55d")
    );
    assert_eq!(
        resolve_outcome(component.as_ref(), FILE_URL).await,
        expected
    );
    assert_eq!(native_host.requests(), component_host.requests());
    assert_eq!(native_host.requests().len(), 2);
}

#[tokio::test]
async fn both_sides_fail_the_same_way_with_the_same_code_and_parameters() {
    let cases: Vec<(&str, Vec<HostHttpResponse>, &str)> = vec![
        (
            "deleted file",
            vec![json(404, GET_INFO_INVALID)],
            "mediafire.file_unavailable",
        ),
        (
            "rate limit",
            vec![json(200, API_ERROR_261)],
            "mediafire.rate_limited",
        ),
        (
            "error page 999",
            vec![
                json(200, GET_INFO),
                html(
                    "https://www.mediafire.com/error.php?errno=999&origin=download",
                    "<html><title>Error</title></html>",
                ),
            ],
            "mediafire.private_file",
        ),
        (
            "threshold",
            vec![json(200, GET_INFO), html(FILE_URL, THRESHOLD)],
            "mediafire.download_limit_reached",
        ),
        (
            "password form",
            vec![json(200, GET_INFO), html(FILE_URL, PASSWORD)],
            "mediafire.password_required",
        ),
        (
            "no direct link",
            vec![
                json(200, GET_INFO),
                html(
                    FILE_URL,
                    "<html><head><title>MediaFire</title></head></html>",
                ),
            ],
            "mediafire.no_direct_link",
        ),
    ];
    // A folder address is not in this list on purpose: the host refuses it before the guest's
    // `resolve` runs (`plugin.url_rejected`, see `both_sides_recognise_the_same_addresses`),
    // while the native side reaches the plugin's own `mediafire.folder_not_file`. Both are
    // `unsupported` and neither makes a request; the code differs by adapter, not by plugin.
    for (case, script, code) in cases {
        let url = FILE_URL;
        let [(native_host, native), (component_host, component)] = both(&script);
        let expected = resolve_outcome(native.as_ref(), url).await;
        assert!(
            expected.contains(&format!("code=Some(\"{code}\")")),
            "{case}: {expected}"
        );
        assert!(
            !expected.contains("url=https://www.mediafire.com/file/"),
            "{case}: the page address must never be the download"
        );
        assert_eq!(
            resolve_outcome(component.as_ref(), url).await,
            expected,
            "{case}"
        );
        assert_eq!(native_host.requests(), component_host.requests(), "{case}");
    }
}

#[tokio::test]
async fn both_sides_check_a_batch_identically() {
    let script = [json(200, GET_INFO_BATCH)];
    let [(native_host, native), (component_host, component)] = both(&script);
    let batch = || CheckRequest {
        urls: vec![
            FILE_URL.parse().expect("URL"),
            "https://www.mediafire.com/?uz9u9zqa0tlk6z7"
                .parse()
                .expect("URL"),
            "https://www.mediafire.com/folder/rww7bhhi0yc1l"
                .parse()
                .expect("URL"),
        ],
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    };
    let expected = format!("{:?}", native.check(batch()).await.expect("checked"));
    assert!(expected.contains("Online"), "{expected}");
    assert!(expected.contains("Offline"), "{expected}");
    assert!(expected.contains("Unknown"), "{expected}");
    assert_eq!(
        format!("{:?}", component.check(batch()).await.expect("checked")),
        expected
    );
    assert_eq!(native_host.requests(), component_host.requests());
}

#[tokio::test]
async fn both_sides_refuse_an_account_check_the_same_way() {
    let [(_, native), (_, component)] = both(&[]);
    let account = AccountId::new();
    let expected = format!("{:?}", native.check_account(account).await);
    assert!(expected.contains("mediafire.no_account"), "{expected}");
    assert_eq!(
        format!("{:?}", component.check_account(account).await),
        expected
    );
    assert_eq!(
        native.hosters(account).await.expect("hosters"),
        component.hosters(account).await.expect("hosters")
    );
}

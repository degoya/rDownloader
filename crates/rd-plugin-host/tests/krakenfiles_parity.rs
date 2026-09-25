//! The KrakenFiles plugin's native fallback and its WebAssembly component answer the same
//! scripted host the same way (RD-103-08, acceptance criterion 3).
//!
//! Both builds compile `plugins/krakenfiles/src/resolver.rs`; what differs is the adapter
//! around it - `plugin_common::native` on one side, `plugin_guest` and the sandbox on the
//! other. This test runs one scenario table through both and compares what came back and
//! what was asked of the host, so a conversion that drops a header, a parameter or a size on
//! one side is a failure here rather than a difference a user finds.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};
use rd_plugin_host::{ComponentResolver, PluginManifest};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(root().join("plugins/krakenfiles/tests/fixtures").join(name))
        .unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

fn manifest() -> PluginManifest {
    toml::from_str(
        &std::fs::read_to_string(root().join("plugins/krakenfiles/manifest.toml"))
            .expect("the krakenfiles manifest"),
    )
    .expect("parse manifest")
}

/// One request as both adapters must agree on it: method, URL, body, and the headers with
/// their names lowercased and sorted.
type Observation = (String, String, String, Vec<(String, String)>);

struct ScriptedHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    captchas: Mutex<usize>,
}

impl ScriptedHost {
    fn new(responses: Vec<HostHttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            captchas: Mutex::new(0),
        })
    }

    /// What the plugin asked, normalised to what both adapters must agree on.
    fn observations(&self) -> Vec<Observation> {
        let requests = self.requests.lock().expect("lock");
        requests
            .iter()
            .map(|request| {
                let mut headers: Vec<(String, String)> = request
                    .headers
                    .iter()
                    .map(|header| {
                        (
                            header.name.to_ascii_lowercase(),
                            header.value_template.clone(),
                        )
                    })
                    .collect();
                headers.sort();
                (
                    request.method.clone(),
                    request.url.to_string(),
                    String::from_utf8_lossy(&request.body).into_owned(),
                    headers,
                )
            })
            .collect()
    }
}

#[async_trait]
impl ResolverHost for ScriptedHost {
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
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "missing scripted response"))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        _challenge: rd_plugin_api::CaptchaChallenge,
        _limit: std::time::Duration,
    ) -> Result<rd_plugin_api::CaptchaAnswer, Failure> {
        *self.captchas.lock().expect("lock") += 1;
        Ok(rd_plugin_api::CaptchaAnswer::Token(
            "turnstile-token".to_owned(),
        ))
    }
}

fn response(status: u16, final_url: &str, content_type: &str, body: &str) -> HostHttpResponse {
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

const PAGE_URL: &str = "https://krakenfiles.com/view/dp3ngkjnsx/file.html";
const POST_URL: &str = "https://krakenfiles.com/download/DP3nGKJNsX";
const DIRECT_LINK: &str =
    "https://dl.krakenfiles.com/force-download/ZGlyZWN0LWxpbmstc3ludGhldGlj?fileHash=DP3nGKJNsX";

fn page() -> HostHttpResponse {
    response(
        200,
        PAGE_URL,
        "text/html; charset=UTF-8",
        &fixture("file-page-2026-09-21.html"),
    )
}

fn error_page() -> HostHttpResponse {
    response(
        404,
        PAGE_URL,
        "text/html; charset=UTF-8",
        &fixture("error-page-2026-09-21.html"),
    )
}

fn captcha_invalid() -> HostHttpResponse {
    response(
        500,
        POST_URL,
        "application/json",
        &fixture("download-captcha-invalid-2026-09-21.json"),
    )
}

fn download_ok() -> HostHttpResponse {
    response(
        200,
        POST_URL,
        "application/json",
        &fixture("download-ok-synthetic.json"),
    )
}

fn probe(status: u16) -> HostHttpResponse {
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

/// What a resolve or check came to, in a shape both adapters can be compared in.
#[derive(Debug, PartialEq)]
enum Outcome {
    Resolved {
        url: String,
        file_name: Option<String>,
        size: Option<u64>,
        headers: Vec<(String, String)>,
    },
    Checked(Vec<(String, rd_core::LinkStatus, Option<String>, Option<u64>)>),
    Failed {
        category: FailureKind,
        code: Option<String>,
        params: Vec<(String, String)>,
    },
}

fn failed(failure: Failure) -> Outcome {
    let mut params: Vec<(String, String)> = failure.params.into_iter().collect();
    params.sort();
    Outcome::Failed {
        category: failure.category,
        code: failure.code,
        params,
    }
}

fn guest() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

async fn resolve_with(resolver: &dyn Resolver, url: &str) -> Outcome {
    match resolver
        .resolve(ResolveRequest {
            url: url.parse().expect("url"),
            client: guest(),
        })
        .await
    {
        Ok(resolved) => Outcome::Resolved {
            url: resolved.url.to_string(),
            file_name: resolved.file_name,
            size: resolved.size.map(rd_core::ByteCount::get),
            headers: resolved
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect(),
        },
        Err(failure) => failed(failure),
    }
}

async fn check_with(resolver: &dyn Resolver, urls: &[&str]) -> Outcome {
    match resolver
        .check(CheckRequest {
            urls: urls.iter().map(|url| url.parse().expect("url")).collect(),
            client: guest(),
        })
        .await
    {
        Ok(results) => Outcome::Checked(
            results
                .into_iter()
                .map(|result| {
                    (
                        result.url.to_string(),
                        result.status,
                        result.file_name,
                        result.size.map(rd_core::ByteCount::get),
                    )
                })
                .collect(),
        ),
        Err(failure) => failed(failure),
    }
}

/// One scenario: the scripted answers, and what to ask.
struct Scenario {
    name: &'static str,
    responses: fn() -> Vec<HostHttpResponse>,
    ask: Ask,
}

enum Ask {
    Resolve(&'static str),
    Check(&'static [&'static str]),
}

const LINK: &str = "https://www.krakenfiles.com/view/DP3nGKJNsX/file.html";

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "the free flow resolves",
            responses: || vec![page(), download_ok(), probe(206)],
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "a deleted file",
            responses: || vec![error_page()],
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "captcha rejected twice",
            responses: || vec![page(), captcha_invalid(), page(), captcha_invalid()],
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "ok without a link",
            responses: || {
                vec![
                    page(),
                    response(
                        200,
                        POST_URL,
                        "application/json",
                        r#"{"status":"ok","url":""}"#,
                    ),
                ]
            },
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "another refusal",
            responses: || {
                vec![
                    page(),
                    response(
                        500,
                        POST_URL,
                        "application/json",
                        r#"{"status":"error","url":"","msg":"Server overloaded"}"#,
                    ),
                ]
            },
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "a foreign download host",
            responses: || {
                vec![
                    page(),
                    response(
                        200,
                        POST_URL,
                        "application/json",
                        r#"{"status":"ok","url":"https://cdn.example.net/file.rar"}"#,
                    ),
                ]
            },
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "a refused direct link",
            responses: || vec![page(), download_ok(), probe(405)],
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "a page without the form",
            responses: || {
                vec![response(
                    200,
                    PAGE_URL,
                    "text/html",
                    "<html><head><title>Maintenance - Krakenfiles.com</title></head></html>",
                )]
            },
            ask: Ask::Resolve(LINK),
        },
        Scenario {
            name: "a link check",
            responses: || {
                vec![
                    response(
                        200,
                        "https://krakenfiles.com/json/dp3ngkjnsx",
                        "application/json",
                        &fixture("json-file-2026-09-21.json"),
                    ),
                    response(
                        200,
                        "https://krakenfiles.com/json/ztpdkgdzy8",
                        "application/json",
                        &fixture("json-missing-2026-09-21.json"),
                    ),
                ]
            },
            ask: Ask::Check(&[
                LINK,
                "https://krakenfiles.com/embed-video/zTpdkgdZY8",
                "https://example.com/view/DP3nGKJNsX/file.html",
            ]),
        },
    ]
}

async fn run(resolver: &dyn Resolver, ask: &Ask) -> Outcome {
    match ask {
        Ask::Resolve(url) => resolve_with(resolver, url).await,
        Ask::Check(urls) => check_with(resolver, urls).await,
    }
}

#[tokio::test]
async fn the_native_fallback_and_the_component_agree_on_every_scenario() {
    let component = rd_plugin_host::artifact::component("rd-plugin-krakenfiles");
    for scenario in scenarios() {
        let native_host = ScriptedHost::new((scenario.responses)());
        let native = rd_plugin_krakenfiles::KrakenfilesResolver::new(native_host.clone());
        let native_outcome = run(&native, &scenario.ask).await;

        let component_host = ScriptedHost::new((scenario.responses)());
        let sandboxed = ComponentResolver::new(manifest(), &component, component_host.clone())
            .expect("the component loads");
        let component_outcome = run(&sandboxed, &scenario.ask).await;

        assert_eq!(
            native_outcome, component_outcome,
            "{}: the two builds answered differently",
            scenario.name
        );
        assert_eq!(
            native_host.observations(),
            component_host.observations(),
            "{}: the two builds asked the host different things",
            scenario.name
        );
        assert_eq!(
            *native_host.captchas.lock().expect("lock"),
            *component_host.captchas.lock().expect("lock"),
            "{}: the two builds asked for a different number of captchas",
            scenario.name
        );
        // Not a tautology: a scenario whose script was never consumed would compare two
        // "missing scripted response" failures and pass.
        assert!(
            native_host.responses.lock().expect("lock").is_empty(),
            "{}: scripted answers were left over",
            scenario.name
        );
    }
}

/// A link the plugin does not claim is refused on both sides before any request is made -
/// but by different code: the sandbox checks `match-url` itself and answers
/// `plugin.url_rejected` without entering the guest, while the native adapter lets the
/// plugin's own `resolve` say `krakenfiles.unsupported_link`. Both are `Unsupported`, and the
/// difference is the host's, not the plugin's, which is why the case is not in the table.
#[tokio::test]
async fn an_unclaimed_link_is_refused_on_both_sides_before_any_request() {
    let component = rd_plugin_host::artifact::component("rd-plugin-krakenfiles");
    let short_form = "https://krakenfiles.com/view/DP3nGKJNsX";

    let native_host = ScriptedHost::new(Vec::new());
    let native = rd_plugin_krakenfiles::KrakenfilesResolver::new(native_host.clone());
    let native_outcome = resolve_with(&native, short_form).await;
    assert_eq!(
        native_outcome,
        Outcome::Failed {
            category: FailureKind::Unsupported,
            code: Some("krakenfiles.unsupported_link".to_owned()),
            params: Vec::new(),
        }
    );

    let component_host = ScriptedHost::new(Vec::new());
    let sandboxed = ComponentResolver::new(manifest(), &component, component_host.clone())
        .expect("the component loads");
    let component_outcome = resolve_with(&sandboxed, short_form).await;
    assert_eq!(
        component_outcome,
        Outcome::Failed {
            category: FailureKind::Unsupported,
            code: Some("plugin.url_rejected".to_owned()),
            params: Vec::new(),
        }
    );

    assert!(native_host.observations().is_empty());
    assert!(component_host.observations().is_empty());
}

/// The success scenario, pinned to its values so the table above cannot quietly agree on a
/// wrong answer.
#[tokio::test]
async fn the_component_resolves_the_measured_page_to_the_direct_link() {
    let component = rd_plugin_host::artifact::component("rd-plugin-krakenfiles");
    let host = ScriptedHost::new(vec![page(), download_ok(), probe(206)]);
    let sandboxed =
        ComponentResolver::new(manifest(), &component, host.clone()).expect("the component loads");

    let outcome = resolve_with(&sandboxed, LINK).await;

    assert_eq!(
        outcome,
        Outcome::Resolved {
            url: DIRECT_LINK.to_owned(),
            file_name: Some("EldenRing_Fix_Repair_Steam_Generic.rar".to_owned()),
            size: Some(5_138_022),
            headers: vec![("Referer".to_owned(), "https://krakenfiles.com/".to_owned())],
        }
    );
    let observations = host.observations();
    assert_eq!(observations.len(), 3, "page, post, probe");
    assert_eq!(observations[1].0, "POST");
    assert_eq!(observations[1].1, POST_URL);
    assert!(
        observations[1]
            .2
            .contains("cf-turnstile-response=turnstile-token"),
        "{}",
        observations[1].2
    );
    assert!(
        observations[1]
            .3
            .contains(&("hash".to_owned(), "DP3nGKJNsX".to_owned())),
        "{:?}",
        observations[1].3
    );
}

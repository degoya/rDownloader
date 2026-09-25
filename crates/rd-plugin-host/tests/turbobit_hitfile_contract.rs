//! Criterion 3 of RD-103-10 and RD-103-11: the native fallback and the signed component of
//! each brand pass the same contract, on the same fixtures, against the same scripted host.
//!
//! Every scenario is run twice — once through `TurbobitResolver`/`HitfileResolver` and once
//! through `ComponentResolver` over the built `.wasm` — and the two runs must agree on the
//! result, on every request they made (method, address, the headers the API needs, the body),
//! on the captchas they asked for and on the waits they took. A component that skipped a step
//! its native twin takes, or the reverse, fails here rather than in the field.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CaptchaChallenge, CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse,
    ResolveRequest, ResolvedHeader, Resolver, ResolverHost,
};
use rd_plugin_host::{ComponentResolver, PluginManifest};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One brand, as both builds of it.
struct Brand {
    package: &'static str,
    directory: &'static str,
    link: &'static str,
    deleted_link: &'static str,
    check_links: &'static [&'static str],
    native: fn(Arc<dyn ResolverHost>) -> Arc<dyn Resolver>,
}

const TURBOBIT: Brand = Brand {
    package: "rd-plugin-turbobit",
    directory: "turbobit",
    link: "https://turb.pw/a1b2c3d4e5f6.html?short_domain=turb.pw",
    deleted_link: "https://turbobit.net/abcdefghijkl.html",
    check_links: &[
        "https://turbobit.net/a1b2c3d4e5f6.html",
        "https://turbobit.net/abcdefghijkl.html",
        "https://turbobit.net/download/free/a1b2c3d4e5f6",
        "https://turb.pw/a1b2c3d4e5f6.html",
    ],
    native: |host| Arc::new(rd_plugin_turbobit::TurbobitResolver::new(host)),
};

const HITFILE: Brand = Brand {
    package: "rd-plugin-hitfile",
    directory: "hitfile",
    link: "https://hil.to/Gh2IjKl",
    deleted_link: "https://hitfile.net/Mn3OpQr",
    check_links: &[
        "https://hitfile.net/Ab1CdEf",
        "https://hitfile.net/Gh2IjKl",
        "https://hitfile.net/Mn3OpQr",
        "https://hitfile.net/Gh2IjKl.html",
        "https://hil.to/Gh2IjKl",
    ],
    native: |host| Arc::new(rd_plugin_hitfile::HitfileResolver::new(host)),
};

impl Brand {
    fn fixture(&self, name: &str) -> String {
        let path = root()
            .join("plugins")
            .join(self.directory)
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn json(&self, status: u16, name: &str) -> HostHttpResponse {
        answer(status, "application/json", &self.fixture(name))
    }

    fn component(&self, host: Arc<dyn ResolverHost>) -> Arc<dyn Resolver> {
        let manifest: PluginManifest = toml::from_str(
            &std::fs::read_to_string(
                root()
                    .join("plugins")
                    .join(self.directory)
                    .join("manifest.toml"),
            )
            .expect("manifest"),
        )
        .expect("parse manifest");
        let component = rd_plugin_host::artifact::component(self.package);
        Arc::new(ComponentResolver::new(manifest, &component, host).expect("component loads"))
    }

    /// The six answers of a complete guest download.
    fn guest_success(&self) -> Vec<HostHttpResponse> {
        vec![
            self.json(200, "download-info-free-2026-09-21.json"),
            self.json(200, "free-init-ok-synthetic.json"),
            self.json(200, "captcha-2026-09-21.json"),
            self.json(200, "free-captcha-delay-synthetic.json"),
            self.json(200, "free-prepare-ok-synthetic.json"),
            self.json(200, "free-start-ok-synthetic.json"),
        ]
    }
}

fn answer(status: u16, content_type: &str, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://app.example.test/api".parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: content_type.to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

/// One request as recorded: method, address, the headers the API needs, the body.
type RequestRecord = (String, String, Vec<(String, String)>, String);

/// What both builds are compared on.
#[derive(Debug, PartialEq)]
struct Trace {
    outcome: Outcome,
    requests: Vec<RequestRecord>,
    captchas: Vec<(String, String, String)>,
    waits: Vec<u32>,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Resolved {
        url: String,
        file_name: Option<String>,
        size: Option<u64>,
        headers: Vec<(String, String)>,
    },
    Checked(Vec<(String, String, Option<String>)>),
    Account {
        valid: bool,
        premium: bool,
        label: String,
        traffic_left: Option<u64>,
    },
    Failed {
        category: FailureKind,
        code: Option<String>,
        params: Vec<(String, String)>,
    },
}

impl From<Failure> for Outcome {
    fn from(failure: Failure) -> Self {
        let mut params: Vec<(String, String)> = failure.params.into_iter().collect();
        params.sort();
        Self::Failed {
            category: failure.category,
            code: failure.code,
            params,
        }
    }
}

/// The scripted host: answers in order, records everything either build asks of it.
struct Script {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    captchas: Mutex<Vec<CaptchaChallenge>>,
    waits: Mutex<Vec<u32>>,
    has_password: bool,
}

impl Script {
    fn new(responses: Vec<HostHttpResponse>, has_password: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            captchas: Mutex::new(Vec::new()),
            waits: Mutex::new(Vec::new()),
            has_password,
        })
    }

    fn trace(&self, outcome: Outcome) -> Trace {
        let requests = self
            .requests
            .lock()
            .expect("lock")
            .iter()
            .map(|request| {
                let mut headers: Vec<(String, String)> = request
                    .headers
                    .iter()
                    .filter(|header| {
                        matches!(
                            header.name.to_ascii_lowercase().as_str(),
                            "accept" | "content-type" | "origin" | "referer"
                        )
                    })
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
                    headers,
                    String::from_utf8_lossy(&request.body).into_owned(),
                )
            })
            .collect();
        let captchas = self
            .captchas
            .lock()
            .expect("lock")
            .iter()
            .map(|challenge| match challenge {
                CaptchaChallenge::Turnstile(widget) => (
                    "turnstile".to_owned(),
                    widget.site_key.clone(),
                    widget.page_url.clone(),
                ),
                other => ("other".to_owned(), format!("{other:?}"), String::new()),
            })
            .collect();
        Trace {
            outcome,
            requests,
            captchas,
            waits: self.waits.lock().expect("lock").clone(),
        }
    }
}

#[async_trait]
impl ResolverHost for Script {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        // The answer comes from the address that was asked, as a real host's does; the
        // component runtime holds `final_url` to the manifest's domains.
        let final_url = request.url.clone();
        self.requests.lock().expect("lock").push(request);
        self.responses
            .lock()
            .expect("lock")
            .pop_front()
            .map(|mut response| {
                response.final_url = final_url;
                response
            })
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "the script ran out of answers"))
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_password && reference.ends_with("_password")
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

/// `code(name=value)` parts, which is what the interface translates.
fn label_summary(parts: &[rd_plugin_api::LabelPart]) -> String {
    parts
        .iter()
        .map(|part| {
            let params: Vec<String> = part
                .params
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            format!("{}({})", part.code, params.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn client(account: bool) -> ClientIdentity {
    ClientIdentity {
        account_id: account.then(AccountId::new),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

async fn resolve_trace(
    resolver: Arc<dyn Resolver>,
    script: &Script,
    url: &str,
    account: bool,
) -> Trace {
    let outcome = match resolver
        .resolve(ResolveRequest {
            url: url.parse().expect("URL"),
            client: client(account),
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
        Err(failure) => failure.into(),
    };
    script.trace(outcome)
}

/// Runs `scenario` through both builds of `brand` and requires identical traces.
async fn both_builds<F, Fut>(
    brand: &Brand,
    name: &str,
    script: impl Fn() -> Arc<Script>,
    scenario: F,
) where
    F: Fn(Arc<dyn Resolver>, Arc<Script>) -> Fut,
    Fut: std::future::Future<Output = Trace>,
{
    let native_script = script();
    let native = scenario(
        (brand.native)(native_script.clone() as Arc<dyn ResolverHost>),
        native_script,
    )
    .await;
    let component_script = script();
    let component = scenario(
        brand.component(component_script.clone() as Arc<dyn ResolverHost>),
        component_script,
    )
    .await;
    assert_eq!(
        native, component,
        "{}: native and component disagree on `{name}`",
        brand.package
    );
}

#[tokio::test]
async fn a_guest_download_is_resolved_identically_by_both_builds() {
    for brand in [&TURBOBIT, &HITFILE] {
        both_builds(
            brand,
            "guest download",
            || Script::new(brand.guest_success(), false),
            |resolver, script| async move { resolve_trace(resolver, &script, brand.link, false).await },
        )
        .await;
        // And the result is the one-shot link, not a page and not a probe of the link.
        let script = Script::new(brand.guest_success(), false);
        let trace =
            resolve_trace(brand.component(script.clone()), &script, brand.link, false).await;
        match trace.outcome {
            Outcome::Resolved { url, headers, .. } => {
                assert!(
                    url.contains("/download/redirect/0123456789abcdef0123456789abcdef/"),
                    "{url}"
                );
                assert!(headers.iter().any(|(name, _)| name == "Referer"));
            }
            other => panic!("expected a download, got {other:?}"),
        }
        assert_eq!(trace.requests.len(), 6);
        assert_eq!(trace.captchas.len(), 1);
        assert_eq!(trace.captchas[0].0, "turnstile");
        assert_eq!(trace.waits.len(), 1);
    }
}

#[tokio::test]
async fn the_measured_refusals_are_reported_identically_by_both_builds() {
    for brand in [&TURBOBIT, &HITFILE] {
        let deleted = || {
            Script::new(
                vec![brand.json(404, "download-info-deleted-404-2026-09-21.json")],
                false,
            )
        };
        both_builds(
            brand,
            "deleted file",
            deleted,
            |resolver, script| async move {
                resolve_trace(resolver, &script, brand.deleted_link, false).await
            },
        )
        .await;
        let direct_hit_fixture = if brand.directory == "turbobit" {
            "free-init-direct-hit-2026-09-21.json"
        } else {
            "free-init-direct-hit-premium-only-2026-09-21.json"
        };
        let blocked = || {
            Script::new(
                vec![
                    brand.json(200, "download-info-free-2026-09-21.json"),
                    brand.json(200, direct_hit_fixture),
                ],
                false,
            )
        };
        both_builds(brand, "guest window", blocked, |resolver, script| async move {
            resolve_trace(resolver, &script, brand.link, false).await
        })
        .await;
        let shell = || {
            Script::new(
                vec![answer(
                    200,
                    "text/html; charset=UTF-8",
                    &brand.fixture("spa-shell-2026-09-21.html"),
                )],
                false,
            )
        };
        both_builds(brand, "spa shell", shell, |resolver, script| async move {
            resolve_trace(resolver, &script, brand.link, false).await
        })
        .await;
        let rejected_twice = || {
            Script::new(
                vec![
                    brand.json(200, "download-info-free-2026-09-21.json"),
                    brand.json(200, "free-init-ok-synthetic.json"),
                    brand.json(200, "captcha-2026-09-21.json"),
                    brand.json(422, "free-captcha-invalid-422-2026-09-21.json"),
                    brand.json(200, "captcha-2026-09-21.json"),
                    brand.json(422, "free-captcha-invalid-422-2026-09-21.json"),
                ],
                false,
            )
        };
        both_builds(brand, "captcha rejected twice", rejected_twice, |resolver, script| async move {
            resolve_trace(resolver, &script, brand.link, false).await
        })
        .await;
    }
    // HitFile's public premium-only file, live.
    let premium_only = || {
        Script::new(
            vec![HITFILE.json(200, "download-info-premium-only-2026-09-21.json")],
            false,
        )
    };
    both_builds(
        &HITFILE,
        "premium only",
        premium_only,
        |resolver, script| async move {
            resolve_trace(resolver, &script, "https://hitfile.net/Ab1CdEf", false).await
        },
    )
    .await;
    let script = premium_only();
    let trace = resolve_trace(
        HITFILE.component(script.clone()),
        &script,
        "https://hitfile.net/Ab1CdEf",
        false,
    )
    .await;
    assert!(
        matches!(trace.outcome, Outcome::Failed { ref code, category: FailureKind::AuthRequired, .. } if code.as_deref() == Some("hitfile.premium_only")),
        "{trace:?}"
    );
    assert!(trace.captchas.is_empty());
}

#[tokio::test]
async fn the_link_check_is_answered_identically_by_both_builds() {
    for brand in [&TURBOBIT, &HITFILE] {
        let fixture = if brand.directory == "turbobit" {
            "links-check-2026-09-21.json"
        } else {
            "links-check-mixed-2026-09-21.json"
        };
        both_builds(
            brand,
            "link check",
            || Script::new(vec![brand.json(200, fixture)], true),
            |resolver, script| async move {
                let outcome = match resolver
                    .check(CheckRequest {
                        urls: brand
                            .check_links
                            .iter()
                            .map(|url| url.parse().expect("URL"))
                            .collect(),
                        client: client(true),
                    })
                    .await
                {
                    Ok(results) => Outcome::Checked(
                        results
                            .into_iter()
                            .map(|result| {
                                (
                                    result.url.to_string(),
                                    format!("{:?}", result.status),
                                    result.file_name,
                                )
                            })
                            .collect(),
                    ),
                    Err(failure) => failure.into(),
                };
                script.trace(outcome)
            },
        )
        .await;
    }
}

#[tokio::test]
async fn the_account_check_is_answered_identically_by_both_builds() {
    // The login half is shared; its synthetic fixtures live with Turbobit.
    let answers = || {
        Script::new(
            vec![
                TURBOBIT.json(401, "user-info-unauthenticated-401-2026-09-21.json"),
                TURBOBIT.json(200, "auth-login-ok-synthetic.json"),
                TURBOBIT.json(200, "user-info-premium-active-synthetic.json"),
                TURBOBIT.json(200, "premium-info-synthetic.json"),
            ],
            true,
        )
    };
    for brand in [&TURBOBIT, &HITFILE] {
        both_builds(
            brand,
            "account check",
            answers,
            |resolver, script| async move {
                let outcome = match resolver.check_account(AccountId::new()).await {
                    Ok(status) => Outcome::Account {
                        valid: status.valid,
                        premium: status.premium,
                        label: label_summary(&status.label),
                        traffic_left: status.traffic_left.map(rd_core::ByteCount::get),
                    },
                    Err(failure) => failure.into(),
                };
                script.trace(outcome)
            },
        )
        .await;
    }
    let script = answers();
    let status = TURBOBIT
        .component(script.clone())
        .check_account(AccountId::new())
        .await
        .expect("checked");
    assert!(status.premium);
    let body = String::from_utf8_lossy(&script.requests.lock().expect("lock")[1].body).into_owned();
    assert!(body.contains("{{secret:turbobit_password}}"), "{body}");
}

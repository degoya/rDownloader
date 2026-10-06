//! The resolver contract, exercised against the signed 1fichier component
//! (RD-191-07, PLUG-23).
//!
//! `onefichier`'s unit tests drive its host-side build (`OneFichierResolver`); nothing ran the
//! WebAssembly component the bundle actually ships. These are the same account-mode scenarios
//! put through `ComponentResolver` over the built `.wasm`, so a guest adapter that drifted from
//! the shared logic — a request it shapes differently, a refusal it reports under another code
//! — fails here. The account-less free flow stays with the unit tests: it is the same shared
//! code behind the same adapter, and its fixtures live with the plugin.
//!
//! There is no native-against-component parity harness here as there is for Turbobit and
//! Hitfile in `rd-plugin-host`: that one needs the plugin crate as a dependency of the host's
//! tests. This covers the component the way the other `rd-plugin-ext` resolver contracts do.
//!
//! **1fichier is a mock.** It answers at the host boundary, so no socket is opened, no API key
//! is needed and none is in this file.

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
use rd_plugin_host::{ComponentResolver, PluginManifest, artifact::component};

const MANIFEST: &str = include_str!("../../../../plugins/onefichier/manifest.toml");
const FILE_INFO: &str = "https://api.1fichier.com/v1/file/info.cgi";
const GET_TOKEN: &str = "https://api.1fichier.com/v1/download/get_token.cgi";
const USER_INFO: &str = "https://api.1fichier.com/v1/user/info.cgi";
const LINK: &str = "https://1fichier.com/?abc12defg3";

/// Answers in order, and records every request as the plugin described it.
struct MockOneFichier {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    has_key: bool,
}

impl MockOneFichier {
    fn new(responses: Vec<HostHttpResponse>, has_key: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_key,
        })
    }

    fn requests(&self) -> Vec<(String, String, Vec<u8>)> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .map(|request| {
                (
                    request.method.clone(),
                    request.url.to_string(),
                    request.body.clone(),
                )
            })
            .collect()
    }
}

#[async_trait]
impl ResolverHost for MockOneFichier {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.lock().expect("requests").push(request);
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "no answer scripted"))
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_key && reference == "onefichier_api_key"
    }
}

fn json(status: u16, url: &str, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn resolver(host: &Arc<MockOneFichier>) -> ComponentResolver {
    let manifest: PluginManifest = toml::from_str(MANIFEST).expect("bundled manifest");
    let bytes = component("rd-plugin-onefichier");
    let host: Arc<dyn ResolverHost> = Arc::clone(host) as Arc<dyn ResolverHost>;
    ComponentResolver::new(manifest, &bytes, host).expect("compile the resolver")
}

fn client() -> ClientIdentity {
    ClientIdentity {
        account_id: Some(AccountId::new()),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn resolve_request() -> ResolveRequest {
    ResolveRequest {
        url: LINK.parse().expect("URL"),
        client: client(),
    }
}

#[tokio::test]
async fn an_account_resolve_asks_for_the_file_then_the_token_with_the_key_as_a_marker() {
    let host = MockOneFichier::new(
        vec![
            json(
                200,
                FILE_INFO,
                r#"{"filename":"release.rar","size":"4096"}"#,
            ),
            json(
                200,
                GET_TOKEN,
                r#"{"status":"OK","url":"https://cdn123.1fichier.com/d/tok/release.rar"}"#,
            ),
        ],
        true,
    );
    let resolved = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        "https://cdn123.1fichier.com/d/tok/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(4096));

    let body = br#"{"url":"https://1fichier.com/?abc12defg3"}"#.to_vec();
    assert_eq!(
        host.requests(),
        vec![
            ("POST".to_owned(), FILE_INFO.to_owned(), body.clone()),
            ("POST".to_owned(), GET_TOKEN.to_owned(), body),
        ]
    );
    let requests = host.requests.lock().expect("requests");
    for request in requests.iter() {
        assert!(
            request.headers.iter().any(|header| {
                header.name == "Authorization"
                    && header.value_template == "Bearer {{secret:onefichier_api_key}}"
            }),
            "the key must leave the plugin as the vault marker, never as a value"
        );
    }
}

#[tokio::test]
async fn an_account_without_a_key_is_refused_before_any_request() {
    let host = MockOneFichier::new(Vec::new(), false);
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("missing key");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("1fichier.api_key_missing"));
    assert!(host.requests().is_empty());
}

/// Each refusal the API states arrives under the code and category the native build reports.
#[tokio::test]
async fn every_stated_refusal_keeps_its_code_and_category() {
    let cases = [
        (
            json(
                200,
                FILE_INFO,
                r#"{"status":"KO","message":"Not authenticated #12"}"#,
            ),
            FailureKind::AccountInvalid,
            "1fichier.bad_api_key",
        ),
        (
            json(
                404,
                FILE_INFO,
                r#"{"status":"KO","message":"Resource not found #469"}"#,
            ),
            // A 404 is a file gone for good, not retried (owner, 2026-10-04, RA-PLG-02).
            FailureKind::Permanent,
            "1fichier.file_offline",
        ),
        (
            json(
                200,
                FILE_INFO,
                r#"{"status":"KO","message":"Flood detected: IP Locked #38"}"#,
            ),
            FailureKind::RateLimited {
                retry_after_seconds: Some(300),
            },
            "1fichier.flood",
        ),
    ];
    for (answer, category, code) in cases {
        let host = MockOneFichier::new(vec![answer], true);
        let failure = resolver(&host)
            .resolve(resolve_request())
            .await
            .expect_err("a refusal");
        assert_eq!(failure.category, category, "{code}");
        assert_eq!(failure.code.as_deref(), Some(code));
    }
}

#[tokio::test]
async fn a_key_without_a_subscription_is_told_it_needs_one() {
    let host = MockOneFichier::new(
        vec![
            json(
                200,
                FILE_INFO,
                r#"{"filename":"release.rar","size":"4096"}"#,
            ),
            json(
                200,
                GET_TOKEN,
                r#"{"status":"KO","message":"Must be a customer (Premium, Access) #200"}"#,
            ),
        ],
        true,
    );
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("premium required");
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("1fichier.premium_required"));
}

#[tokio::test]
async fn a_malformed_download_address_is_refused_by_the_component_too() {
    let host = MockOneFichier::new(
        vec![
            json(
                200,
                FILE_INFO,
                r#"{"filename":"release.rar","size":"4096"}"#,
            ),
            json(200, GET_TOKEN, r#"{"status":"OK","url":"not a url"}"#),
        ],
        true,
    );
    let failure = resolver(&host)
        .resolve(resolve_request())
        .await
        .expect_err("malformed download URL");
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("1fichier.invalid_url"));
}

#[tokio::test]
async fn the_account_check_reads_the_offer_and_the_traffic() {
    let host = MockOneFichier::new(
        vec![json(
            200,
            USER_INFO,
            r#"{"email":"user@example.test","offer":1,"subscription_end":"2027-01-01","cdn":"2"}"#,
        )],
        true,
    );
    let status = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("status");
    assert!(status.valid);
    assert!(status.premium);
    assert_eq!(
        status.traffic_left.map(|bytes| bytes.get()),
        Some(2 * 1024 * 1024 * 1024)
    );
    assert_eq!(
        host.requests(),
        vec![("POST".to_owned(), USER_INFO.to_owned(), b"{}".to_vec())]
    );
}

/// A batch answers every link: an offline one is `Offline`, one that is not a file address is
/// `Unknown` without a request, and a refused key degrades to `Unknown` rather than failing
/// the batch.
#[tokio::test]
async fn the_link_check_answers_every_link_of_the_batch() {
    let host = MockOneFichier::new(
        vec![
            json(200, FILE_INFO, r#"{"filename":"a.rar","size":"10"}"#),
            json(
                404,
                FILE_INFO,
                r#"{"status":"KO","message":"Resource not found #1"}"#,
            ),
        ],
        true,
    );
    let results = resolver(&host)
        .check(CheckRequest {
            urls: vec![
                LINK.parse().expect("URL"),
                "https://1fichier.com/?deadbeef99".parse().expect("URL"),
                "https://1fichier.com/not-a-file-link".parse().expect("URL"),
            ],
            client: client(),
        })
        .await
        .expect("results");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("a.rar"));
    assert_eq!(results[1].status, LinkStatus::Offline);
    assert_eq!(results[2].status, LinkStatus::Unknown);
    assert_eq!(host.requests().len(), 2);

    let refused = MockOneFichier::new(
        vec![json(
            200,
            FILE_INFO,
            r#"{"status":"KO","message":"Not authenticated #12"}"#,
        )],
        true,
    );
    let results = resolver(&refused)
        .check(CheckRequest {
            urls: vec![LINK.parse().expect("URL")],
            client: client(),
        })
        .await
        .expect("results");
    assert_eq!(results[0].status, LinkStatus::Unknown);
}

#[test]
fn the_component_reaches_only_1fichiers_domains_and_its_own_key() {
    let manifest: PluginManifest = toml::from_str(MANIFEST).expect("bundled manifest");
    let http = manifest
        .capabilities
        .net_http
        .as_ref()
        .expect("the resolver needs HTTP");
    assert!(
        http.domains
            .iter()
            .any(|domain| domain == "api.1fichier.com")
    );
    assert!(http.domains.iter().all(|domain| {
        let domain = domain.trim_start_matches("*.");
        domain == "api.1fichier.com"
            || [
                "1fichier.com",
                "alterupload.com",
                "cjoint.net",
                "desfichiers.com",
                "megadl.fr",
                "mesfichiers.org",
                "dl4free.com",
                "tenvoi.com",
                "piecejointe.net",
                "pjointe.com",
            ]
            .contains(&domain)
    }));
    assert_eq!(manifest.capabilities.secrets, ["onefichier_api_key"]);
}

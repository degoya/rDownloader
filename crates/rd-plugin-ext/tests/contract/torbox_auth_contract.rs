//! The authentication contract, exercised against the bundled TorBox key check
//! (RD-191-07, PLUG-23).
//!
//! `torbox-auth` had unit tests of how it reads one answer and nothing that ran the component.
//! What only the component can show: that the key leaves the plugin as the vault marker and
//! never as a value, that a key nobody stored is answered without a request, that a refused key
//! ends the flow while "not now" keeps it waiting, and that the wait TorBox states is the one
//! the host is told — bounded by the shared one-day ceiling.
//!
//! **TorBox is a mock.** It answers at the host boundary, so no socket is opened and
//! api.torbox.app is not contacted; no key of anybody's is in this file.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    PluginManifest,
    artifact::component,
    extension::{AuthProgress, AuthProvider},
};

const MANIFEST: &str = include_str!("../../../../plugins/torbox-auth/manifest.toml");

/// One canned answer, whether a key is stored, and a record of every request.
struct MockTorBox {
    key_stored: bool,
    status: u16,
    retry_after: Option<&'static str>,
    body: &'static str,
    /// Every request, as `<method> <url>`.
    requests: Mutex<Vec<String>>,
    /// Every `Authorization` value the plugin sent, verbatim.
    authorizations: Mutex<Vec<String>>,
}

impl MockTorBox {
    fn answering(status: u16, body: &'static str) -> Self {
        Self {
            key_stored: true,
            status,
            retry_after: None,
            body,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ResolverHost for MockTorBox {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
        }
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {}", request.method, request.url));
        Ok(HostHttpResponse {
            status: self.status,
            final_url: request.url.clone(),
            headers: self
                .retry_after
                .map(|value| ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: value.to_owned(),
                })
                .into_iter()
                .collect(),
            body: self.body.as_bytes().to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.key_stored && reference == "torbox_api_key"
    }
}

fn provider(host: &Arc<MockTorBox>) -> AuthProvider {
    let manifest: PluginManifest = toml::from_str(MANIFEST).expect("bundled manifest");
    let bytes = component("rd-plugin-torbox-auth");
    let host: Arc<dyn ResolverHost> = Arc::clone(host) as Arc<dyn ResolverHost>;
    AuthProvider::new(manifest, &bytes, Some(host)).expect("compile against the auth world")
}

#[tokio::test]
async fn a_key_the_account_answers_for_signs_in_and_never_leaves_as_a_value() {
    let host = Arc::new(MockTorBox::answering(
        200,
        r#"{"success":true,"detail":"ok","data":{"plan":2}}"#,
    ));
    let provider = provider(&host);
    assert_eq!(
        provider.begin(AccountId::new(), None).await.expect("begin"),
        AuthProgress::Authorized
    );
    // `poll` is the same question: a check of a stored key has nothing to remember.
    assert_eq!(
        provider.poll(AccountId::new(), None).await.expect("poll"),
        AuthProgress::Authorized
    );
    assert_eq!(
        host.requests(),
        vec![
            "GET https://api.torbox.app/v1/api/user/me".to_owned(),
            "GET https://api.torbox.app/v1/api/user/me".to_owned(),
        ]
    );
    let authorizations = host.authorizations.lock().expect("authorizations").clone();
    assert!(
        authorizations
            .iter()
            .all(|value| value == "Bearer {{secret:torbox_api_key}}"),
        "the plugin must name the reference, never hold the key: {authorizations:?}"
    );
}

/// TorBox answers a bad key with a `200` and a word; the word decides, and the flow ends.
#[tokio::test]
async fn a_refused_key_ends_the_flow() {
    let host = Arc::new(MockTorBox::answering(
        200,
        r#"{"success":false,"error":"BAD_TOKEN","detail":"invalid api key abc123"}"#,
    ));
    match provider(&host)
        .begin(AccountId::new(), None)
        .await
        .expect("begin")
    {
        AuthProgress::Failed { message } => {
            // The provider's sentence, which here echoes part of a key, never travels.
            assert!(!message.contains("abc123"), "{message}");
        }
        other => panic!("expected the flow to fail, got {other:?}"),
    }
}

/// Asking TorBox about a key nobody stored would spend the account's request budget to learn
/// what the installation already knows.
#[tokio::test]
async fn a_key_nobody_stored_is_answered_without_a_request() {
    let host = Arc::new(MockTorBox {
        key_stored: false,
        ..MockTorBox::answering(200, "{}")
    });
    let progress = provider(&host)
        .begin(AccountId::new(), None)
        .await
        .expect("begin");
    assert!(
        matches!(progress, AuthProgress::Failed { .. }),
        "{progress:?}"
    );
    assert!(host.requests().is_empty(), "{:?}", host.requests());
}

/// "Not now" is a wait, never a refusal, and TorBox's own figure is the wait — up to the
/// one-day ceiling every plugin shares (RD-191-07, PLUG-12).
#[tokio::test]
async fn a_rate_limit_waits_for_the_stated_time_within_one_day() {
    for (stated, expected) in [
        (Some("120"), 120),
        (Some("31536000"), 86_400),
        // No usable figure: the plugin's own default for a rate limit, which is not zero.
        (Some("0"), 60),
        (Some("Wed, 21 Oct 2015 07:28:00 GMT"), 60),
        (None, 60),
    ] {
        let host = Arc::new(MockTorBox {
            retry_after: stated,
            ..MockTorBox::answering(429, "")
        });
        assert_eq!(
            provider(&host)
                .begin(AccountId::new(), None)
                .await
                .expect("begin"),
            AuthProgress::Pending {
                retry_after_seconds: expected
            },
            "{stated:?}"
        );
    }
}

/// An outage is not evidence about anybody's key: the flow waits rather than failing.
#[tokio::test]
async fn an_outage_is_a_wait_and_not_a_refusal() {
    let host = Arc::new(MockTorBox::answering(503, "<html>maintenance</html>"));
    let progress = provider(&host)
        .begin(AccountId::new(), None)
        .await
        .expect("begin");
    assert!(
        matches!(progress, AuthProgress::Pending { retry_after_seconds } if retry_after_seconds > 0),
        "{progress:?}"
    );
}

#[test]
fn the_plugin_reaches_only_torbox_and_reads_only_its_own_key() {
    let manifest: PluginManifest = toml::from_str(MANIFEST).expect("bundled manifest");
    let http = manifest
        .capabilities
        .net_http
        .as_ref()
        .expect("the check needs HTTP");
    assert_eq!(http.domains, ["api.torbox.app"]);
    assert_eq!(manifest.capabilities.secrets, ["torbox_api_key"]);
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
}

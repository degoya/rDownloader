//! pCloud's sign-in, which shares no flow with the other cloud drives: no PKCE, no grant type, a
//! client secret on the exchange, nothing to renew with, and a code that has to be offered to
//! the installation that issued it. Its mock authorization server is two installations too.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    PluginManifest,
    extension::{OAuthProvider, TokenOutcome},
};

use super::{EU_API, PROVIDER_PROSE, US_API};
use crate::common::oauth::{Recorded, Stored};
use crate::support::storage::{sign_in::PLACEHOLDER_ACCESS_TOKEN, table::PCLOUD};

const TOKEN_SUCCESS: &str = include_str!("../../fixtures/pcloud/token_success.json");
const TOKEN_INVALID_CODE: &str = include_str!("../../fixtures/pcloud/token_invalid_code.json");
const TOKEN_RATE_LIMITED: &str = include_str!("../../fixtures/pcloud/token_rate_limited.json");

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-pcloud-oauth")
}

/// The mock pCloud authorization server, in two installations.
struct MockAuthorizer {
    /// The installation that issued the code, and the only one that will redeem it.
    home: &'static str,
    /// When set, both installations answer this instead.
    refusal: Option<&'static str>,
    /// Whether the account carries a registered application.
    registered: bool,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockAuthorizer {
    fn new(home: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            refusal: None,
            registered: true,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn refusing(body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            refusal: Some(body),
            registered: true,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn unregistered() -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            refusal: None,
            registered: false,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn stored(&self) -> Vec<Stored> {
        self.stored.lock().expect("stored").clone()
    }
}

#[async_trait]
impl ResolverHost for MockAuthorizer {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let form: Vec<(String, String)> = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            url: request.url.to_string(),
            form,
        });
        let host = request.url.host_str().unwrap_or_default().to_owned();
        let body = self.refusal.unwrap_or(if host == self.home {
            TOKEN_SUCCESS
        } else {
            // The installation that did not issue this code does not know it.
            TOKEN_INVALID_CODE
        });
        Ok(HostHttpResponse {
            // pCloud answers HTTP 200 to a refused exchange too.
            status: 200,
            final_url: request.url.clone(),
            headers: vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
            body: body.as_bytes().to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.registered || reference != "pcloud_client_secret"
    }

    async fn store_oauth_token(
        &self,
        account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        crate::common::oauth::store(
            &self.stored,
            account_id,
            access_token,
            refresh_token,
            expires_in_seconds,
        )
    }
}

fn provider(host: Arc<MockAuthorizer>, bytes: &[u8]) -> OAuthProvider {
    let manifest: PluginManifest =
        toml::from_str(PCLOUD.shape.sign_in).expect("the oauth manifest");
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

fn parameter(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The authorization code, from the address the person is sent to all the way to the stored
/// token — and stored without renewal material, because pCloud issues none.
#[tokio::test]
async fn the_sign_in_runs_through_to_a_stored_token() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(US_API);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    assert!(
        request
            .authorization_url
            .starts_with("https://my.pcloud.com/oauth2/authorize?"),
        "{}",
        request.authorization_url
    );
    assert_eq!(
        parameter(&request.authorization_url, "response_type").as_deref(),
        Some("code")
    );
    assert_eq!(
        parameter(&request.authorization_url, "state").as_deref(),
        Some(request.state.as_str())
    );
    // No application key anywhere in this plugin: the marker travels and the host substitutes
    // what *this installation* registered (RD-106-04, rule 8).
    assert!(
        request
            .authorization_url
            .contains("client_id=%7B%7Bclient_id%7D%7D")
            || request
                .authorization_url
                .contains("client_id={{client_id}}"),
        "the authorization URL must carry the marker, not an application key: {}",
        request.authorization_url
    );
    // pCloud offers no PKCE, so there is deliberately nothing to carry between the two calls,
    // and no challenge is invented to look like there is.
    assert!(!request.authorization_url.contains("code_challenge"));
    assert_eq!(request.flow_state, None);
    assert!(request.state.len() >= 43);

    let outcome = plugin
        .poll(account, "the-code-the-callback-carried", None)
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, "https://api.pcloud.com/oauth2_token");
    assert_eq!(
        exchange.field("code"),
        Some("the-code-the-callback-carried")
    );
    // The markers, and never values. The mock stands where the host's expansion would be, so
    // what it sees is what the plugin wrote.
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    assert_eq!(
        exchange.field("client_secret"),
        Some("{{secret:pcloud_client_secret}}")
    );

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            // pCloud issues neither, and saying so is what keeps the host's renewal sweep away
            // from an account it could never renew.
            refresh_token: None,
            expires_in_seconds: None,
        }]
    );
}

/// **The region, in the sign-in.** pCloud states the account's data centre in the redirect,
/// but the contract hands `poll` the code alone. So the code is offered to one installation
/// and then the other, and a code the first never issued cannot be spent there.
#[tokio::test]
async fn a_code_issued_in_the_other_installation_is_redeemed_there() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(EU_API);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .poll(account, "a-european-code", None)
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);
    assert_eq!(
        server
            .requests()
            .iter()
            .map(|request| request.url.clone())
            .collect::<Vec<_>>(),
        vec![
            "https://api.pcloud.com/oauth2_token".to_owned(),
            "https://eapi.pcloud.com/oauth2_token".to_owned(),
        ]
    );
    assert_eq!(server.stored().len(), 1);
}

/// A code neither installation will take ends the flow, and it took both of them to say so.
#[tokio::test]
async fn a_code_neither_installation_takes_ends_the_flow() {
    let bytes = oauth_component();
    let server = MockAuthorizer::refusing(TOKEN_INVALID_CODE);
    let outcome = provider(server.clone(), &bytes)
        .poll(AccountId::new(), "a-stale-code", None)
        .await
        .expect("poll");
    let TokenOutcome::Failed { message, .. } = outcome else {
        panic!("{outcome:?}");
    };
    // pCloud's own number survives; the sentence beside it does not.
    assert!(message.contains("2094"), "{message}");
    assert!(!message.contains(PROVIDER_PROSE), "{message}");
    assert_eq!(server.requests().len(), 2);
    assert!(server.stored().is_empty(), "nothing may be stored");
}

/// A rate limit is a wait, not a failure, and carries pCloud's own `Retry-After`.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = oauth_component();
    let server = MockAuthorizer::refusing(TOKEN_RATE_LIMITED);
    let outcome = provider(server.clone(), &bytes)
        .poll(AccountId::new(), "a-code", None)
        .await
        .expect("poll");
    assert_eq!(
        outcome,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );
    assert!(server.stored().is_empty(), "a rate limit stores nothing");
    assert_eq!(
        server.requests().len(),
        1,
        "a rate limit is not a region question"
    );
}

/// An account with no registered application is told so before anything is asked of pCloud.
///
/// Without this the person would be sent looking for a fault in a registration they never made.
#[tokio::test]
async fn an_account_without_a_registered_application_is_refused_before_anything_is_asked() {
    let bytes = oauth_component();
    let server = MockAuthorizer::unregistered();
    let failure = provider(server.clone(), &bytes)
        .poll(AccountId::new(), "a-code", None)
        .await
        .expect_err("no application");
    assert!(
        format!("{failure:?}").contains("client_not_configured"),
        "{failure:?}"
    );
    assert!(server.requests().is_empty());
}

/// There is nothing to renew from, and the plugin says so rather than pretending.
#[tokio::test]
async fn a_renewal_is_refused_because_pcloud_issues_nothing_to_renew_with() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(US_API);
    let outcome = provider(server.clone(), &bytes)
        .refresh(AccountId::new(), Some("pcloud_access_token"))
        .await
        .expect("refresh");
    let TokenOutcome::Failed { message, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(message.contains("no renewal material"), "{message}");
    assert!(
        server.requests().is_empty(),
        "a renewal that cannot exist must ask pCloud nothing"
    );
}

/// The device entrance is not offered, and being asked anyway is a stable refusal rather than
/// a trap. pCloud has no device flow at all.
#[tokio::test]
async fn the_device_entrance_is_refused_with_a_stable_code() {
    let bytes = oauth_component();
    let server = MockAuthorizer::new(US_API);
    let failure = provider(server.clone(), &bytes)
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("no device entrance");
    assert!(
        format!("{failure:?}").contains("flow_unsupported"),
        "{failure:?}"
    );
    assert!(
        server.requests().is_empty(),
        "a flow that is not offered must ask pCloud nothing"
    );
}

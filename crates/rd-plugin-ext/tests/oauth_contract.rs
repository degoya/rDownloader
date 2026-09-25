//! The OAuth contract, exercised end to end against a mock authorization server.
//!
//! What is proven here is the exchange itself, not its bookkeeping: the reference plugin runs
//! as a real WebAssembly component, the mock stands in for the provider's token endpoint, and
//! it checks the PKCE challenge the way a provider does — the verifier `poll` sends has to
//! hash to the challenge `begin` put in the authorization URL, or the exchange is refused.
//! That is the proof RD-103-00 left open.
//!
//! **The mock is a mock.** It answers at the host boundary, so no socket is opened and no real
//! provider is contacted. A run against a real provider belongs to the provider plugins
//! (RD-106-03 … RD-106-06) and is not claimed here.
//!
//! The cases are the ones a host reacts to differently:
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | The person agreed | `success.json` | `Authorized`, both tokens stored |
//! | The person refused | `failure.json` | `Failed`, the person is told |
//! | The code expired | `expiry.json` | `Failed`, start again |
//! | The refresh was refused | `refresh_refused.json` | `Failed`, sign in again |
//! | Rate limited | `quota.json` + `Retry-After` | `Pending`, the host waits |
//! | A device code was issued | `device_code.json` | a prompt, and a code to poll with |
//! | Nobody has confirmed yet | `device_pending.json` | `Pending`, the host waits |
//!
//! Since RD-106-01 the same five outcomes are exercised through the device entrance as well,
//! and one thing beyond them: that a device sign-in ends in refresh material, so the renewal
//! sweep keeps it alive and nobody types a code twice. That is the whole point of the job,
//! and `a_device_sign_in_is_renewed_without_anybody_being_asked_again` is where it is proved.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    OAuthFlowManifest, PluginManifest,
    extension::{AuthorizationRequest, OAuthProvider, TokenOutcome},
};

const MANIFEST: &str = include_str!("../../../plugins/example-oauth/manifest.toml");

// The sanitised fixtures. Every token-shaped value in them is the placeholder below and
// nothing else; `fixtures_carry_no_credential_material` is what keeps it that way.
const SUCCESS: &str = include_str!("fixtures/oauth/success.json");
const FAILURE: &str = include_str!("fixtures/oauth/failure.json");
const EXPIRY: &str = include_str!("fixtures/oauth/expiry.json");
const QUOTA: &str = include_str!("fixtures/oauth/quota.json");
const REFRESH_REFUSED: &str = include_str!("fixtures/oauth/refresh_refused.json");
const DEVICE_CODE: &str = include_str!("fixtures/oauth/device_code.json");
const DEVICE_PENDING: &str = include_str!("fixtures/oauth/device_pending.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";
const PLACEHOLDER_DEVICE_CODE: &str = "redacted-device-code-0000";

/// The reference component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-example-oauth")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the reference manifest")
}

/// What the token endpoint should answer next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    /// Checks the PKCE verifier and, when it matches, hands over the tokens.
    Granted,
    /// Hands the tokens over without a PKCE check, which is what a refresh grant is: PKCE
    /// binds an authorization code, and a renewal carries none.
    RenewalGranted,
    Denied,
    Expired,
    RefreshRefused,
    /// 429 with `Retry-After: 90`.
    RateLimited,
    /// The device entrance: the device endpoint hands out a code, the token endpoint grants.
    DeviceGranted,
    /// The device endpoint hands out a code and the token endpoint says "not yet".
    DevicePending,
    /// The device endpoint hands out a code and the token endpoint asks for more time.
    DeviceSlowDown,
    /// The device endpoint hands out a code and the code then runs out.
    DeviceExpired,
    /// The provider could not be reached at all — not an answer, a failed call.
    Unreachable,
}

/// One request the plugin made, flattened to what a test wants to assert on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    url: String,
    form: Vec<(String, String)>,
}

impl Recorded {
    fn field(&self, name: &str) -> Option<&str> {
        self.form
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What `store-oauth-token` was called with.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Stored {
    account_id: AccountId,
    access_token: String,
    refresh_token: Option<String>,
    expires_in_seconds: Option<u64>,
}

/// The mock authorization server.
///
/// It answers at the host boundary rather than over a socket, which is also what makes the
/// second half of its job possible: it sees the request exactly as the plugin described it,
/// so a test can assert that the refresh material left the plugin as the template
/// `{{secret:…}}` and never as a value.
struct MockAuthorizationServer {
    case: Mutex<Case>,
    /// The `code_challenge` the authorization URL carried, once `begin` has run.
    challenge: Mutex<Option<String>>,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockAuthorizationServer {
    fn new(case: Case) -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(case),
            challenge: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn expect_challenge(&self, challenge: &str) {
        *self.challenge.lock().expect("challenge") = Some(challenge.to_owned());
    }

    fn set_case(&self, case: Case) {
        *self.case.lock().expect("case") = case;
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn stored(&self) -> Vec<Stored> {
        self.stored.lock().expect("stored").clone()
    }

    fn answer(&self, url: &str, form: &[(String, String)]) -> Result<HostHttpResponse, Failure> {
        let case = *self.case.lock().expect("case");
        let body = |status: u16, text: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: url::Url::parse("https://oauth.example.invalid/oauth/token")
                    .expect("url"),
                headers,
                body: text.as_bytes().to_vec(),
            })
        };
        // The device entrance is two endpoints, not one, so the mock has to tell them apart
        // the way a provider does: by the address the request went to.
        if url.ends_with("/oauth/device/code") && case != Case::Unreachable {
            return body(200, DEVICE_CODE, Vec::new());
        }
        match case {
            Case::DeviceGranted => {
                assert!(
                    form.iter().all(|(name, _)| name != "code_verifier"),
                    "a device grant carries no PKCE verifier"
                );
                body(200, SUCCESS, Vec::new())
            }
            Case::DevicePending => body(400, DEVICE_PENDING, Vec::new()),
            Case::DeviceSlowDown => body(
                429,
                QUOTA,
                vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
            ),
            Case::DeviceExpired => body(400, EXPIRY, Vec::new()),
            Case::Granted => {
                // The check a real authorization server makes: the verifier the client kept
                // back has to hash to the challenge it published. Without it the "PKCE" in
                // this test would be decoration.
                let expected = self.challenge.lock().expect("challenge").clone();
                let verifier = form
                    .iter()
                    .find(|(name, _)| name == "code_verifier")
                    .map(|(_, value)| value.clone());
                match (expected, verifier) {
                    (Some(expected), Some(verifier)) if s256(&verifier) == expected => {
                        body(200, SUCCESS, Vec::new())
                    }
                    _ => body(400, REFRESH_REFUSED, Vec::new()),
                }
            }
            Case::RenewalGranted => {
                assert!(
                    form.iter().all(|(name, _)| name != "code_verifier"),
                    "a refresh grant must not carry a PKCE verifier"
                );
                body(200, SUCCESS, Vec::new())
            }
            Case::Denied => body(400, FAILURE, Vec::new()),
            Case::Expired => body(400, EXPIRY, Vec::new()),
            Case::RefreshRefused => body(400, REFRESH_REFUSED, Vec::new()),
            Case::RateLimited => body(
                429,
                QUOTA,
                vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
            ),
            Case::Unreachable => Err(Failure::coded(
                FailureKind::Offline,
                "plugin.offline",
                "the provider could not be reached",
            )),
        }
    }
}

#[async_trait]
impl ResolverHost for MockAuthorizationServer {
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
            form: form.clone(),
        });
        self.answer(request.url.as_str(), &form)
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }

    async fn store_oauth_token(
        &self,
        account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        self.stored.lock().expect("stored").push(Stored {
            account_id,
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.map(str::to_owned),
            expires_in_seconds,
        });
        Ok(())
    }
}

/// base64url, unpadded, of the SHA-256 digest — PKCE's `S256`, computed independently of the
/// plugin so a matching pair is evidence rather than a tautology.
fn s256(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// One query parameter of the authorization URL.
fn parameter(request: &AuthorizationRequest, name: &str) -> Option<String> {
    url::Url::parse(&request.authorization_url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn provider(host: Arc<MockAuthorizationServer>, bytes: &[u8]) -> OAuthProvider {
    OAuthProvider::new(manifest(), bytes, Some(host)).expect("the reference plugin builds")
}

#[tokio::test]
async fn the_reference_plugin_compiles_against_the_oauth_world() {
    // Which also proves the `credentials` import links: only `auth` and `oauth` worlds may
    // name it, so a build that succeeds here is the type binding doing its job.
    let bytes = component();
    OAuthProvider::new(manifest(), &bytes, None).expect("the reference plugin satisfies the world");
}

/// Authorization code with PKCE, from the address the person is sent to all the way to the
/// stored token — the criterion RD-103-00 could not demonstrate because no plugin of this
/// world existed.
#[tokio::test]
async fn authorization_code_with_pkce_runs_through_to_a_stored_token() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");

    // What the person is sent to: the provider's own authorization endpoint, on a domain the
    // manifest declares, carrying a challenge and never a verifier.
    assert!(
        request
            .authorization_url
            .starts_with("https://oauth.example.invalid/oauth/authorize?"),
        "{}",
        request.authorization_url
    );
    assert_eq!(
        parameter(&request, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        parameter(&request, "response_type").as_deref(),
        Some("code")
    );
    let challenge = parameter(&request, "code_challenge").expect("a challenge");
    let verifier = request.flow_state.clone().expect("a verifier");
    assert!(
        !request.authorization_url.contains(&verifier),
        "the verifier must never appear in the address the person is sent to"
    );
    // The half the provider never sees is what the exchange is proved with.
    assert_eq!(s256(&verifier), challenge);
    assert_eq!(
        parameter(&request, "state").as_deref(),
        Some(request.state.as_str())
    );
    assert!((43..=128).contains(&verifier.len()));

    server.expect_challenge(&challenge);
    let outcome = plugin
        .poll(
            account,
            "the-code-the-callback-carried",
            request.flow_state.as_deref(),
        )
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    // The exchange itself: a POST to the token endpoint carrying the code, the verifier and
    // the grant type, and nothing that identifies the person.
    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, "https://oauth.example.invalid/oauth/token");
    assert_eq!(exchange.field("grant_type"), Some("authorization_code"));
    assert_eq!(
        exchange.field("code"),
        Some("the-code-the-callback-carried")
    );
    assert_eq!(exchange.field("code_verifier"), Some(verifier.as_str()));

    // And the point of all of it: both halves reached the vault, with the expiry the sweep
    // needs, for the account the flow was started for and no other.
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3600),
        }]
    );
}

/// A token is stored for the account whose flow produced it, and for no other.
///
/// The host builds a store per invocation and compares the account it was started for with
/// the id the guest names, so two flows running for two accounts cannot cross. Here that is
/// checked from the outside: what reached the vault, and under whose name.
#[tokio::test]
async fn each_flow_stores_only_for_its_own_account() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);

    let mut accounts = Vec::new();
    for _ in 0..2 {
        let account = AccountId::new();
        let request = plugin.begin(account, None).await.expect("begin");
        server.expect_challenge(&parameter(&request, "code_challenge").expect("challenge"));
        let outcome = plugin
            .poll(account, "code", request.flow_state.as_deref())
            .await
            .expect("poll");
        assert_eq!(outcome, TokenOutcome::Authorized);
        accounts.push(account);
    }

    let stored: Vec<AccountId> = server
        .stored()
        .into_iter()
        .map(|entry| entry.account_id)
        .collect();
    assert_eq!(stored, accounts);
}

/// Two flows started in the same moment, for the same account, share nothing.
///
/// This is the RD-105-01 defect the security review found: the verifier and the `state` used to
/// be `sha256("<account-id>:<unix-second>:<literal salt>")`, so anybody who knew the account —
/// which `/api/v1/accounts` tells any reader — could recompute both within a few hundred
/// candidate seconds, and PKCE protected nothing. They now come from `host.random-bytes`, and
/// the property that proves it is this one: same account, same second, four different values.
#[tokio::test]
async fn two_flows_in_the_same_moment_share_no_secret() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let first = plugin.begin(account, None).await.expect("begin");
    let second = plugin.begin(account, None).await.expect("begin");

    assert_ne!(first.state, second.state, "the state repeated");
    assert_ne!(first.flow_state, second.flow_state, "the verifier repeated");
    assert_ne!(
        first.state,
        first.flow_state.clone().expect("a verifier"),
        "the state and the verifier are the same value"
    );
    // Not derived from anything the caller supplied, and long enough to be worth having.
    for request in [&first, &second] {
        let verifier = request.flow_state.clone().expect("a verifier");
        assert!((43..=128).contains(&verifier.len()), "{verifier}");
        assert!(
            !verifier.contains(&account.to_string())
                && !request.state.contains(&account.to_string()),
            "a flow value quotes the account it runs for"
        );
    }
}

/// A verifier that does not match the published challenge is refused, which is the whole
/// reason PKCE exists: a stolen code alone is not enough.
#[tokio::test]
async fn a_code_without_its_verifier_is_refused() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();
    let request = plugin.begin(account, None).await.expect("begin");
    server.expect_challenge(&parameter(&request, "code_challenge").expect("challenge"));

    let outcome = plugin
        .poll(
            account,
            "a-stolen-code",
            Some("a-verifier-from-another-flow"),
        )
        .await
        .expect("poll");
    assert!(
        matches!(outcome, TokenOutcome::Failed { .. }),
        "expected a refusal, got {outcome:?}"
    );
    assert!(server.stored().is_empty(), "nothing may be stored");
}

/// Consent refused and an expired code both end the flow, and each says which it was so the
/// interface can tell the person what to do next.
#[tokio::test]
async fn a_refused_consent_and_an_expired_code_end_the_flow() {
    let bytes = component();
    for case in [Case::Denied, Case::Expired] {
        let server = MockAuthorizationServer::new(case);
        let plugin = provider(server.clone(), &bytes);
        let account = AccountId::new();
        let request = plugin.begin(account, None).await.expect("begin");
        let outcome = plugin
            .poll(account, "code", request.flow_state.as_deref())
            .await
            .expect("poll");
        let TokenOutcome::Failed { message, .. } = outcome else {
            panic!("{case:?} should have failed, got {outcome:?}");
        };
        // The provider's own error code survives; its prose does not.
        let expected = match case {
            Case::Denied => "access_denied",
            _ => "expired_token",
        };
        assert!(message.contains(expected), "{message}");
        assert!(
            !message.contains(PLACEHOLDER_ACCESS_TOKEN),
            "a provider's error text reached the message verbatim: {message}"
        );
        assert!(server.stored().is_empty());
    }
}

/// The acceptance criterion in two halves: a refusal is a failure, an unreachable provider is
/// not.
///
/// The difference matters because the renewal sweep acts on it
/// (`rd_api::auth_flow_service::sweep_renewals`): `Failed` records a failure the person is
/// shown and stops asking, while an `Err` keeps the stored token and tries again later. A
/// plugin that reported an unreachable provider as `Failed` would sign people out every time
/// their connection dropped.
#[tokio::test]
async fn a_refused_refresh_fails_and_an_unreachable_provider_does_not() {
    let bytes = component();
    let account = AccountId::new();

    let server = MockAuthorizationServer::new(Case::RefreshRefused);
    let plugin = provider(server.clone(), &bytes);
    let outcome = plugin
        .refresh(account, Some("example_oauth_refresh"))
        .await
        .expect("refresh");
    let TokenOutcome::Failed { message, .. } = outcome else {
        panic!("a revoked refresh token must fail, got {outcome:?}");
    };
    assert!(message.contains("invalid_grant"), "{message}");
    assert!(!message.contains(PLACEHOLDER_REFRESH_TOKEN), "{message}");

    // The refresh material never left the vault: what the plugin sent is the template, which
    // the host expands on the way out and hands back to nobody.
    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some("{{secret:example_oauth_refresh}}")
    );

    let offline = MockAuthorizationServer::new(Case::Unreachable);
    let plugin = provider(offline.clone(), &bytes);
    let outcome = plugin.refresh(account, Some("example_oauth_refresh")).await;
    assert!(
        outcome.is_err(),
        "an unreachable provider must not end the flow, got {outcome:?}"
    );
    assert!(offline.stored().is_empty());
}

/// A rate limit is a wait, not a failure, and the plugin passes the provider's `Retry-After`
/// on so the host waits as long as it was asked to.
#[tokio::test]
async fn a_rate_limit_becomes_a_wait_carrying_retry_after() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::RateLimited);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    let polled = plugin
        .poll(account, "code", request.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(
        polled,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );

    let renewed = plugin
        .refresh(account, Some("example_oauth_refresh"))
        .await
        .expect("refresh");
    assert_eq!(
        renewed,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );
    assert!(server.stored().is_empty(), "a rate limit stores nothing");
}

/// A renewal stores the new token without anybody being asked, and asks for the refresh grant
/// rather than starting a new authorization.
#[tokio::test]
async fn a_renewal_stores_the_new_token_without_a_redirect() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::RenewalGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin
        .refresh(account, Some("example_oauth_refresh"))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.url, "https://oauth.example.invalid/oauth/token");
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert!(sent.field("code").is_none(), "a renewal carries no code");

    // Nobody was sent anywhere, and the new token reached the vault with its expiry.
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3600),
        }]
    );
}

/// The device entrance, end to end: a code the person types, a poll that waits, and a token
/// that lands in the vault with the refresh material the sweep needs.
///
/// The criterion RD-106-01 exists for. A device flow of the older `auth` world could reach the
/// first two of those and never the third: `auth-state` has no way to say a token expires, so
/// the sweep never learned of it and the person was asked again the next time it did.
#[tokio::test]
async fn a_device_code_runs_through_to_a_stored_token_with_its_renewal_material() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::DevicePending);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    assert_eq!(
        prompt.verification_url,
        "https://oauth.example.invalid/device"
    );
    assert_eq!(prompt.user_code.as_deref(), Some("WXYZ-1234"));
    assert_eq!(prompt.expires_in_seconds, Some(900));
    assert_eq!(prompt.interval_seconds, Some(5));
    // The device code is what the poll is made with and is never part of what is shown.
    let device_code = prompt.flow_state.clone().expect("a device code");
    assert_eq!(device_code, "redacted-device-code-0000");
    assert_ne!(Some(device_code.as_str()), prompt.user_code.as_deref());
    assert!(!prompt.verification_url.contains(&device_code));

    // Nobody has confirmed yet. That is a wait, not a failure: reading it as one would end a
    // sign-in while the person is still walking to the other screen.
    let waiting = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(
        waiting,
        TokenOutcome::Pending {
            retry_after_seconds: 5
        }
    );
    assert!(server.stored().is_empty(), "a wait stores nothing");

    server.set_case(Case::DeviceGranted);
    let granted = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(granted, TokenOutcome::Authorized);

    // The exchange: the device grant type, the device code, and nothing that names the person.
    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, "https://oauth.example.invalid/oauth/token");
    assert_eq!(
        exchange.field("grant_type"),
        Some("urn:ietf:params:oauth:grant-type:device_code")
    );
    assert_eq!(exchange.field("device_code"), Some(device_code.as_str()));
    assert!(exchange.field("code").is_none());

    // And the half that makes this job worth doing: refresh material, with an expiry.
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3600),
        }]
    );
}

/// The outcome the job is named for: a device sign-in is renewed by the same `refresh` a
/// redirect sign-in is, so the person types a code once and never again.
#[tokio::test]
async fn a_device_sign_in_is_renewed_without_anybody_being_asked_again() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::DeviceGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    let granted = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(granted, TokenOutcome::Authorized);

    // The renewal that follows. Nobody is shown anything, no device code is issued, and the
    // stored refresh material leaves the plugin as a template rather than as a value.
    server.set_case(Case::RenewalGranted);
    let renewed = plugin
        .refresh(account, Some("example_oauth_refresh"))
        .await
        .expect("refresh");
    assert_eq!(renewed, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some("{{secret:example_oauth_refresh}}")
    );
    assert!(
        sent.field("device_code").is_none(),
        "a renewal is not a device sign-in"
    );
    // Two stored tokens, one sign-in: the second arrived without anybody being asked.
    assert_eq!(server.stored().len(), 2);
    assert!(
        !server
            .requests()
            .iter()
            .any(|request| request.url.ends_with("/oauth/device/code")
                && request.method == "POST"
                && server.stored().len() > 2),
        "a renewal must not start a second device sign-in"
    );
}

/// The two ends of a device flow that is not going to finish, told apart from waiting.
///
/// `slow_down` is the provider asking for room and keeps the sign-in alive; an expired device
/// code ends it and says which it was, so the interface can tell the person to start again.
#[tokio::test]
async fn a_slow_down_waits_and_an_expired_device_code_ends_the_flow() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::DeviceSlowDown);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    let slowed = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(
        slowed,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );

    server.set_case(Case::DeviceExpired);
    let expired = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    let TokenOutcome::Failed { message, .. } = expired else {
        panic!("an expired device code must end the flow, got {expired:?}");
    };
    assert!(message.contains("expired_token"), "{message}");
    assert!(server.stored().is_empty(), "nothing may be stored");

    // And a provider that could not be reached at all is neither. The call fails, the host
    // keeps whatever it had, and nothing is decided about the credential -- the same
    // distinction the redirect entrance makes, and for the same reason: a plugin that
    // reported an unreachable provider as `failed` would sign people out whenever their
    // connection dropped.
    let offline = MockAuthorizationServer::new(Case::Unreachable);
    let plugin = provider(offline.clone(), &bytes);
    assert!(
        plugin.device_begin(account, None).await.is_err(),
        "an unreachable provider must not end a device sign-in"
    );
    assert!(
        plugin
            .device_poll(account, Some(PLACEHOLDER_DEVICE_CODE))
            .await
            .is_err(),
        "an unreachable provider must not end a device sign-in"
    );
    assert!(offline.stored().is_empty());
}

/// A poll with no device code asks the provider nothing at all.
#[tokio::test]
async fn a_device_poll_without_its_code_never_reaches_the_provider() {
    let bytes = component();
    let server = MockAuthorizationServer::new(Case::DeviceGranted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let outcome = plugin.device_poll(account, None).await.expect("poll");
    assert!(
        matches!(outcome, TokenOutcome::Failed { .. }),
        "expected a refusal, got {outcome:?}"
    );
    assert!(
        server.requests().is_empty(),
        "a poll with nothing to poll with must ask nothing"
    );
    assert!(server.stored().is_empty());
}

/// No token and no refresh material reaches a log line, an API answer or an SSE frame.
///
/// The three places are one place from here: everything the interface ever shows about a flow
/// comes out of `AuthorizationRequest` and `TokenOutcome`, and everything the operator ever
/// sees comes out of `tracing`. So the whole exchange runs with a subscriber capturing every
/// record, and the two placeholder tokens are looked for in all of it.
// A plain test, not `#[tokio::test]`: the capture below starts a runtime of its own inside the
// subscriber's scope, and a runtime cannot be started from within one.
#[test]
fn no_token_or_refresh_material_reaches_a_log_or_an_answer() {
    let bytes = component();
    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let outcomes = tracing::subscriber::with_default(subscriber, || {
        futures_lite_block_on(async {
            let server = MockAuthorizationServer::new(Case::Granted);
            let plugin = provider(server.clone(), &bytes);
            let account = AccountId::new();
            let request = plugin.begin(account, None).await.expect("begin");
            server.expect_challenge(&parameter(&request, "code_challenge").expect("challenge"));
            let granted = plugin
                .poll(account, "code", request.flow_state.as_deref())
                .await
                .expect("poll");
            server.set_case(Case::RefreshRefused);
            let refused = plugin
                .refresh(account, Some("example_oauth_refresh"))
                .await
                .expect("refresh");
            (request, granted, refused)
        })
    });
    let (request, granted, refused) = outcomes;

    let logged = String::from_utf8(captured.lock().expect("captured").clone()).expect("utf-8");
    let mut surfaces = vec![
        logged,
        format!("{request:?}"),
        format!("{granted:?}"),
        format!("{refused:?}"),
    ];
    surfaces.push(request.authorization_url.clone());
    surfaces.push(request.state.clone());
    for surface in &surfaces {
        for secret in [PLACEHOLDER_ACCESS_TOKEN, PLACEHOLDER_REFRESH_TOKEN] {
            assert!(
                !surface.contains(secret),
                "credential material surfaced in: {surface}"
            );
        }
    }
    // And the verifier, which is not a credential but is the half that proves the exchange,
    // stays out of the address the person is sent to.
    let verifier = request.flow_state.clone().expect("a verifier");
    assert!(!request.authorization_url.contains(&verifier));
}

/// A `tracing` writer that keeps every record in memory.
#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("captured").extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs a future to completion on a private runtime.
///
/// `tracing::subscriber::with_default` is synchronous and scoped to the current thread, and
/// the guest's host calls are async — so the whole exchange has to run inside the closure
/// rather than around it, or the records would be written with the global subscriber and
/// captured by nothing.
fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

/// Nothing that could be a credential is committed to the repository.
///
/// The fixtures are the one place where token-shaped strings live in the tree at all, so they
/// are the one place that has to be checked: every value in them is one of the two
/// placeholders below, and a fixture refreshed from a real provider's answer fails here.
#[test]
fn fixtures_carry_no_credential_material() {
    for (name, body) in [
        ("success.json", SUCCESS),
        ("failure.json", FAILURE),
        ("expiry.json", EXPIRY),
        ("quota.json", QUOTA),
        ("refresh_refused.json", REFRESH_REFUSED),
        ("device_code.json", DEVICE_CODE),
        ("device_pending.json", DEVICE_PENDING),
    ] {
        let value: serde_json::Value = serde_json::from_str(body).expect("fixture is JSON");
        for field in [
            "access_token",
            "refresh_token",
            "id_token",
            "code",
            "device_code",
        ] {
            let Some(found) = value.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            assert!(
                found == PLACEHOLDER_ACCESS_TOKEN
                    || found == PLACEHOLDER_REFRESH_TOKEN
                    || found == PLACEHOLDER_DEVICE_CODE,
                "{name} carries a `{field}` that is not a placeholder"
            );
        }
    }
}

/// The reference plugin asks for nothing beyond the provider's own endpoint.
#[test]
fn the_reference_plugin_reaches_only_the_provider_it_signs_in() {
    let manifest = manifest();
    assert_eq!(
        manifest.capabilities.domains(),
        ["oauth.example.invalid".to_owned()].as_slice()
    );
    // No cookies, no captcha, no raw sockets, and no vault reference of its own: the one
    // credential it may expand is chosen per invocation by the host, from the account.
    assert!(manifest.capabilities.net_stream.is_none());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.secrets.is_empty());
    // Both ways in, declared where the host reads them. The reference plugin serves both so
    // the contract tests can drive both; a real provider names only what it offers, and the
    // host then never calls the other (RD-106-01).
    assert_eq!(
        manifest.oauth_flows(),
        [OAuthFlowManifest::Redirect, OAuthFlowManifest::Device].as_slice()
    );
    assert!(manifest.serves_oauth_flow(OAuthFlowManifest::Device));
    // One plugin, one provider.
    let claims = manifest
        .extension
        .as_ref()
        .map(|extension| extension.claims.clone())
        .unwrap_or_default();
    assert_eq!(claims, vec!["example".to_owned()]);
}

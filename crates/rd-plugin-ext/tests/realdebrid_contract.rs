//! The Real-Debrid sign-in contract, driven end to end against a mock of the provider
//! (RD-106-03).
//!
//! The plugin runs as a real WebAssembly component and the mock stands in for
//! `api.real-debrid.com`. It answers at the host boundary, so no socket is opened, no account
//! is needed and no request leaves the machine — which is also what makes the second half of
//! its job possible: it sees each request exactly as the plugin described it, so a test can
//! assert that the refresh material left the plugin as the template `{{secret:…}}` and never
//! as a value.
//!
//! The cases are the ones a host reacts to differently:
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A device code was issued | `device_code.json` | a prompt, and a code to poll with |
//! | Nobody has confirmed yet | `pending.json` | `Pending`, the host waits |
//! | The request budget is spent | `quota.json` + `Retry-After` | `Pending`, the host waits |
//! | The person agreed | `success.json` | `Authorized`, both tokens stored |
//! | The person refused | `refused.json` | `Failed`, the person is told |
//! | The code expired | `expiry.json` | `Failed`, start again |
//! | The registration is refused | `client_rejected.json` | `Failed`, under its own code |
//! | The renewal was refused | `refresh_refused.json` | `Failed`, sign in again |
//!
//! **A run against the real provider is not claimed here.** It needs a Real-Debrid account and
//! an application registration; `docs/roadmap/jobs/archive/106-03-real-debrid.md` records which
//! acceptance criteria that leaves unproven.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{
    OAuthFlowManifest, PluginManifest,
    extension::{OAuthProvider, TokenOutcome},
};

const MANIFEST: &str = include_str!("../../../plugins/realdebrid-auth/manifest.toml");

// The sanitised fixtures. Every token-shaped value in them is a placeholder below and nothing
// else; `fixtures_carry_no_credential_material` is what keeps it that way.
const DEVICE_CODE: &str = include_str!("fixtures/realdebrid/device_code.json");
const SUCCESS: &str = include_str!("fixtures/realdebrid/success.json");
const PENDING: &str = include_str!("fixtures/realdebrid/pending.json");
const QUOTA: &str = include_str!("fixtures/realdebrid/quota.json");
const EXPIRY: &str = include_str!("fixtures/realdebrid/expiry.json");
const REFUSED: &str = include_str!("fixtures/realdebrid/refused.json");
const CLIENT_REJECTED: &str = include_str!("fixtures/realdebrid/client_rejected.json");
const REFRESH_REFUSED: &str = include_str!("fixtures/realdebrid/refresh_refused.json");

const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";
const PLACEHOLDER_DEVICE_CODE: &str = "redacted-device-code-0000";

const DEVICE_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/device/code";
const TOKEN_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/token";
/// Real-Debrid's own grant type, which is not the RFC 8628 URN.
const GRANT_TYPE: &str = "http://oauth.net/grant_type/device/1.0";

/// The plugin component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-realdebrid-auth")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// What the token endpoint should answer next. The device endpoint is told apart by address,
/// the way a provider tells them apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    Granted,
    /// Nobody has confirmed the code yet.
    Pending,
    /// `error_code` 5 with a `Retry-After`.
    SlowDown,
    Expired,
    Denied,
    ClientRejected,
    RefreshRefused,
    /// The provider could not be reached at all — not an answer, a failed call.
    Unreachable,
    /// The device endpoint answers something the plugin cannot read.
    DeviceUnreadable,
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

struct MockRealDebrid {
    case: Mutex<Case>,
    /// Whether the account carries a registered application at all.
    registered: bool,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockRealDebrid {
    fn new(case: Case) -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(case),
            registered: true,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    /// An account that has not registered an application.
    fn unregistered() -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(Case::Granted),
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

    fn answer(&self, url: &str) -> Result<HostHttpResponse, Failure> {
        let case = *self.case.lock().expect("case");
        let body = |status: u16, text: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: url::Url::parse(url).expect("url"),
                headers,
                body: text.as_bytes().to_vec(),
            })
        };
        if case == Case::Unreachable {
            return Err(Failure::coded(
                FailureKind::Offline,
                "plugin.offline",
                "the provider could not be reached",
            ));
        }
        if url.starts_with(DEVICE_ENDPOINT) {
            return match case {
                Case::DeviceUnreadable => body(200, r#"{"error":"missing parameter"}"#, Vec::new()),
                _ => body(200, DEVICE_CODE, Vec::new()),
            };
        }
        match case {
            Case::Granted => body(200, SUCCESS, Vec::new()),
            Case::Pending => body(400, PENDING, Vec::new()),
            Case::SlowDown => body(
                429,
                QUOTA,
                vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
            ),
            Case::Expired => body(400, EXPIRY, Vec::new()),
            Case::Denied => body(403, REFUSED, Vec::new()),
            Case::ClientRejected => body(401, CLIENT_REJECTED, Vec::new()),
            Case::RefreshRefused => body(401, REFRESH_REFUSED, Vec::new()),
            Case::Unreachable | Case::DeviceUnreadable => unreachable!("handled above"),
        }
    }
}

#[async_trait]
impl ResolverHost for MockRealDebrid {
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
        self.answer(request.url.as_str())
    }

    /// The account has registered an application unless a test says it has not. What the
    /// plugin asks here is the one question `secret-available` answers: does a credential
    /// exist — never what it is.
    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "realdebrid_client_secret" && self.registered
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

fn provider(host: Arc<MockRealDebrid>, bytes: &[u8]) -> OAuthProvider {
    OAuthProvider::new(manifest(), bytes, Some(host)).expect("the plugin builds")
}

#[tokio::test]
async fn the_plugin_compiles_against_the_oauth_world() {
    // Which also proves the `credentials` import links: only the `auth` and `oauth` worlds may
    // name it, so a build that succeeds here is the type binding doing its job.
    let bytes = component();
    OAuthProvider::new(manifest(), &bytes, None).expect("the plugin satisfies the world");
}

/// The whole point of the job, in one test: a code typed on another screen ends in a stored
/// token *and* in refresh material, so the renewal sweep keeps the account alive and nobody is
/// ever asked to type a second code.
#[tokio::test]
async fn a_device_sign_in_runs_through_to_a_stored_token_and_is_then_renewed() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    let authorization = plugin
        .device_begin(account, None)
        .await
        .expect("device_begin");

    // What the person is shown: the provider's own page, on a domain the manifest declares,
    // and a short code to type there.
    assert_eq!(
        authorization.verification_url,
        "https://real-debrid.com/device"
    );
    assert_eq!(authorization.user_code.as_deref(), Some("WXYZ1234"));
    assert_eq!(authorization.expires_in_seconds, Some(1800));
    assert_eq!(authorization.interval_seconds, Some(5));
    // The device code is what the poll is made with and is never shown, so it travels in
    // `flow-state` and nowhere else.
    assert_eq!(
        authorization.flow_state.as_deref(),
        Some(PLACEHOLDER_DEVICE_CODE)
    );
    assert_ne!(
        authorization.user_code.as_deref(),
        authorization.flow_state.as_deref()
    );

    let outcome = plugin
        .device_poll(account, authorization.flow_state.as_deref())
        .await
        .expect("device_poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, TOKEN_ENDPOINT);
    assert_eq!(exchange.field("grant_type"), Some(GRANT_TYPE));
    assert_eq!(exchange.field("code"), Some(PLACEHOLDER_DEVICE_CODE));
    // The person's own registration, named and never held: the host substitutes both values
    // on the way out, and a build that shipped a pair would be publishing it.
    assert_eq!(exchange.field("client_id"), Some("{{username}}"));
    assert_eq!(
        exchange.field("client_secret"),
        Some("{{secret:realdebrid_client_secret}}")
    );
    // A device grant carries no PKCE verifier: there is no authorization code to bind.
    assert!(exchange.field("code_verifier").is_none());

    // Both halves reached the vault, with the expiry the renewal sweep needs, for the account
    // the flow was started for and no other.
    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3600),
        }]
    );

    // And the renewal that follows, which is why this is an `oauth` plugin at all: the same
    // grant type, the stored material in `code`, and the person asked nothing.
    let renewed = plugin
        .refresh(account, Some("account/renewal/reference"))
        .await
        .expect("refresh");
    assert_eq!(renewed, TokenOutcome::Authorized);
    let renewal = server.requests().pop().expect("a renewal request");
    assert_eq!(renewal.url, TOKEN_ENDPOINT);
    assert_eq!(renewal.field("grant_type"), Some(GRANT_TYPE));
    // The material left the plugin as a template. The plugin never held the value, and the
    // host is what substitutes it on the way out.
    assert_eq!(
        renewal.field("code"),
        Some("{{secret:account/renewal/reference}}")
    );
    assert_eq!(server.stored().len(), 2);
}

/// Waiting is not refusal. A sign-in that read "not yet" as an ending would die while the
/// person was still walking to the other screen.
#[tokio::test]
async fn an_unconfirmed_code_keeps_the_sign_in_open() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::Pending);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();
    let authorization = plugin
        .device_begin(account, None)
        .await
        .expect("device_begin");

    let outcome = plugin
        .device_poll(account, authorization.flow_state.as_deref())
        .await
        .expect("device_poll");
    assert!(
        matches!(
            outcome,
            TokenOutcome::Pending {
                retry_after_seconds: 5
            }
        ),
        "{outcome:?}"
    );
    assert!(server.stored().is_empty());
}

/// A spent request budget is the same answer, with the provider's own figure. Real-Debrid
/// counts refused requests towards the very cap that refused them, so asking faster is the one
/// thing that must not happen.
#[tokio::test]
async fn a_spent_request_budget_waits_the_time_the_provider_stated() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::SlowDown);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();
    let authorization = plugin
        .device_begin(account, None)
        .await
        .expect("device_begin");

    let outcome = plugin
        .device_poll(account, authorization.flow_state.as_deref())
        .await
        .expect("device_poll");
    assert!(
        matches!(
            outcome,
            TokenOutcome::Pending {
                retry_after_seconds: 90
            }
        ),
        "{outcome:?}"
    );
}

/// The refusals, each under its own code, and none of them quoting the provider.
#[tokio::test]
async fn every_refusal_ends_the_sign_in_under_its_own_code() {
    let bytes = component();
    for (case, expected) in [
        (Case::Denied, "access_denied"),
        (Case::Expired, "expired_token"),
        (Case::ClientRejected, "invalid_client"),
    ] {
        let server = MockRealDebrid::new(case);
        let plugin = provider(server.clone(), &bytes);
        let account = AccountId::new();
        let authorization = plugin
            .device_begin(account, None)
            .await
            .expect("device_begin");
        let outcome = plugin
            .device_poll(account, authorization.flow_state.as_deref())
            .await
            .expect("device_poll");
        let TokenOutcome::Failed { category, message } = outcome else {
            panic!("{case:?} should have failed");
        };
        // The provider's own RFC error word survives, because it is the part that is safe.
        assert!(message.contains(expected), "{case:?}: {message}");
        // Its sentence does not — and `refused.json` carries a token inside that sentence,
        // which is the whole reason the rule exists.
        assert!(!message.contains(PLACEHOLDER_ACCESS_TOKEN), "{case:?}");
        assert!(!message.contains("7f3c9ab2"), "{case:?}");
        // A refusal the provider actually made, so the sweep gives the sign-in up rather than
        // holding a token through it.
        assert_eq!(category, FailureKind::AuthRequired, "{case:?}");
        assert!(server.stored().is_empty(), "{case:?}");
    }
}

/// A refused renewal is a refusal and not a wait, so the sweep stops asking and the person is
/// told to sign in again.
#[tokio::test]
async fn a_refused_renewal_ends_the_stored_sign_in() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::RefreshRefused);
    let plugin = provider(server.clone(), &bytes);
    let outcome = plugin
        .refresh(AccountId::new(), Some("account/renewal/reference"))
        .await
        .expect("refresh");
    let TokenOutcome::Failed { message, .. } = outcome else {
        panic!("a revoked renewal should fail, got {outcome:?}");
    };
    assert!(message.contains("invalid_grant"), "{message}");
    assert!(!message.contains(PLACEHOLDER_REFRESH_TOKEN), "{message}");
}

/// A provider that cannot be reached is **not** a refusal. The stored token is kept and the
/// host tries again later; confusing the two would sign people out whenever a connection
/// dropped.
#[tokio::test]
async fn an_unreachable_provider_is_an_error_and_never_a_refusal() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::Unreachable);
    let plugin = provider(server.clone(), &bytes);
    plugin
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("an unreachable provider is an error");
    assert!(server.stored().is_empty());
}

/// A prompt nobody could act on is not a prompt: the sign-in fails at once rather than showing
/// an empty screen and polling a code that does not exist.
#[tokio::test]
async fn an_unreadable_device_answer_fails_before_anybody_is_shown_anything() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::DeviceUnreadable);
    let plugin = provider(server.clone(), &bytes);
    plugin
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("an unreadable device answer is a failure");
}

/// A renewal with nothing stored refuses instead of asking the provider a question with an
/// empty answer in it.
#[tokio::test]
async fn a_renewal_without_stored_material_never_reaches_the_provider() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let outcome = plugin
        .refresh(AccountId::new(), None)
        .await
        .expect("refresh");
    assert!(
        matches!(outcome, TokenOutcome::Failed { .. }),
        "a renewal with nothing to renew should fail, got {outcome:?}"
    );
    assert!(server.requests().is_empty());
}

/// The redirect entrance the manifest does not offer answers with a stable code rather than a
/// trap. The host refuses first — `require_flow` reads the manifest before it calls anything —
/// so this is the belt beneath that brace.
#[tokio::test]
async fn the_redirect_entrance_refuses_with_a_stable_code() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();
    let failure = plugin
        .begin(account, None)
        .await
        .expect_err("this provider has no redirect entrance");
    assert!(
        format!("{failure:#}").contains("device code"),
        "{failure:#}"
    );
    plugin
        .poll(account, "code", None)
        .await
        .expect_err("this provider has no redirect entrance");
    assert!(server.requests().is_empty());
}

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    for (name, body) in [
        ("device_code.json", DEVICE_CODE),
        ("success.json", SUCCESS),
        ("pending.json", PENDING),
        ("quota.json", QUOTA),
        ("expiry.json", EXPIRY),
        ("refused.json", REFUSED),
        ("client_rejected.json", CLIENT_REJECTED),
        ("refresh_refused.json", REFRESH_REFUSED),
    ] {
        let value: serde_json::Value = serde_json::from_str(body).expect("fixture is JSON");
        for field in [
            "access_token",
            "refresh_token",
            "id_token",
            "code",
            "device_code",
            "client_secret",
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

/// The plugin asks for nothing beyond the provider it signs in, and offers only the entrance
/// that provider has.
#[test]
fn the_plugin_reaches_only_the_provider_it_signs_in() {
    let manifest = manifest();
    assert_eq!(
        manifest.capabilities.domains(),
        [
            "api.real-debrid.com".to_owned(),
            "real-debrid.com".to_owned(),
            "www.real-debrid.com".to_owned()
        ]
        .as_slice()
    );
    // No cookies, no captcha, no raw sockets, and no vault reference of its own: the one
    // credential it may expand is chosen per invocation by the host, from the account.
    assert!(manifest.capabilities.net_stream.is_none());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    // One grant, and it is the person's own registration. Nothing else.
    assert_eq!(
        manifest.capabilities.secrets,
        ["realdebrid_client_secret".to_owned()]
    );
    // Device code and nothing else, so the host never asks for a redirect this provider has
    // no endpoint for.
    assert_eq!(
        manifest.oauth_flows(),
        [OAuthFlowManifest::Device].as_slice()
    );
    assert!(!manifest.serves_oauth_flow(OAuthFlowManifest::Redirect));
    // One plugin, one provider — the same slug the resolver beside it declares.
    let claims = manifest
        .extension
        .as_ref()
        .map(|extension| extension.claims.clone())
        .unwrap_or_default();
    assert_eq!(claims, vec!["realdebrid".to_owned()]);
}

/// The address a person is sent to is on a domain the manifest declares. The host enforces
/// this itself; asserting it here says the plugin does not depend on being caught.
#[test]
fn the_sign_in_page_is_on_a_declared_domain() {
    let prompt: serde_json::Value = serde_json::from_str(DEVICE_CODE).expect("fixture is JSON");
    let url = prompt
        .get("verification_url")
        .and_then(serde_json::Value::as_str)
        .expect("a verification address");
    let host = url::Url::parse(url)
        .expect("a URL")
        .host_str()
        .expect("a host")
        .to_owned();
    assert!(
        manifest()
            .capabilities
            .domains()
            .iter()
            .any(|domain| domain == &host),
        "{host} is not declared"
    );
}

/// An account with no registered application is refused before a request is made, and the
/// refusal says what to do rather than that something is missing.
///
/// Without this the provider would answer `invalid_client` — which says the application was
/// refused, not that there is none — and the person would go looking for a fault in a
/// registration they never made.
#[tokio::test]
async fn an_account_without_a_registered_application_never_reaches_the_provider() {
    let bytes = component();
    let server = MockRealDebrid::unregistered();
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();

    plugin
        .device_begin(account, None)
        .await
        .expect_err("there is no application to sign in with");
    plugin
        .device_poll(account, Some("a-device-code"))
        .await
        .expect_err("there is no application to poll with");
    plugin
        .refresh(account, Some("account/renewal/reference"))
        .await
        .expect_err("there is no application to renew with");

    assert!(
        server.requests().is_empty(),
        "nothing may be asked of the provider without a registration"
    );
    assert!(server.stored().is_empty());
}

/// The provider row says what the person supplies and what the sign-in obtains, and they are
/// two slots rather than one. One value cannot be both, and the sign-in used to overwrite the
/// other (RD-106-03).
#[test]
fn the_provider_keeps_the_registration_and_the_token_apart() {
    const RESOLVER: &str = include_str!("../../../plugins/realdebrid/manifest.toml");
    let resolver: PluginManifest = toml::from_str(RESOLVER).expect("the resolver manifest");
    let spec = rd_plugin_host::provider_spec_from_manifest(&resolver).expect("a provider row");
    let person = spec.spec.person_secret_slot().expect("what is typed");
    let flow = spec.spec.flow_secret_slot().expect("what is obtained");
    assert_eq!(person.reference, "realdebrid_client_secret");
    assert_eq!(flow.reference, "realdebrid_access_token");
    // Both stay on the provider's own host, and the account's username -- the client id --
    // is held to the same list.
    assert_eq!(person.domains, ["api.real-debrid.com".to_owned()]);
    assert_eq!(flow.domains, ["api.real-debrid.com".to_owned()]);
    assert!(
        resolver
            .provider
            .as_ref()
            .is_some_and(|p| p.username_required)
    );
}

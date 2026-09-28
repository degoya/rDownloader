//! The Real-Debrid sign-in contract, driven end to end against a mock of the provider
//! (RD-106-03, open-source device flow since RD-150-09).
//!
//! The plugin runs as a real WebAssembly component and the mock stands in for
//! `api.real-debrid.com`. It answers at the host boundary, so no socket is opened, no account
//! is needed and no request leaves the machine — which is also what makes the second half of
//! its job possible: it sees each request exactly as the plugin described it, so a test can
//! assert that the personal client and the refresh material left the plugin as templates
//! `{{secret:…}}` and never as values.
//!
//! The cases are the ones a host reacts to differently:
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A device code was issued | `device_code.json` | a prompt, and a code to poll with |
//! | Nobody has confirmed yet | `pending.json`, `credentials_pending.json` | `Pending`, the host waits |
//! | The request budget is spent | `quota.json` + `Retry-After` | `Pending`, the host waits |
//! | The person agreed | `credentials.json`, `success.json` | `Authorized`, client and tokens stored |
//! | The person refused | `refused.json` | `Failed`, the person is told |
//! | The code expired | `expiry.json` | `Failed`, start again |
//! | The personal client is refused | `client_rejected.json` | `Failed`, under its own code |
//! | The renewal was refused | `refresh_refused.json` | `Failed`, sign in again |
//! | Too many requests to begin | `quota.json` at the device endpoint | an error coded `rate_limited` |
//!
//! The token endpoint is as strict as Real-Debrid's: its four fields are read from an
//! `application/x-www-form-urlencoded` body alone, and a request that carries them anywhere else
//! is answered `parameter_missing` -- the answer 1.5.1 met after every confirmed device.
//!
//! **A run against the real provider is not claimed here.** It needs a Real-Debrid account;
//! `docs/roadmap/jobs/150-09-realdebrid-device-flow-open-source.md` records which acceptance
//! criteria that leaves unproven.

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
const CREDENTIALS: &str = include_str!("fixtures/realdebrid/credentials.json");
const CREDENTIALS_PENDING: &str = include_str!("fixtures/realdebrid/credentials_pending.json");
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
const PLACEHOLDER_CLIENT_ID: &str = "redacted-client-id-0000";
const PLACEHOLDER_CLIENT_SECRET: &str = "redacted-client-secret-0000";

const DEVICE_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/device/code";
const CREDENTIALS_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/device/credentials";
const TOKEN_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/token";
/// Real-Debrid's own grant type, which is not the RFC 8628 URN.
const GRANT_TYPE: &str = "http://oauth.net/grant_type/device/1.0";
/// Real-Debrid's public client id for open-source applications. Not a secret.
const PUBLIC_CLIENT_ID: &str = "X245A4XAIBGVM";

/// The plugin component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-realdebrid-auth")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// What the provider should answer next. The three endpoints are told apart by address, the
/// way a provider tells them apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    Granted,
    /// Nobody has confirmed the code yet, in the RFC word.
    Pending,
    /// Nobody has confirmed the code yet, in an undocumented error of Real-Debrid's own.
    PendingUnnamed,
    /// `error_code` 5 with a `Retry-After`, at the credentials endpoint.
    SlowDown,
    Expired,
    Denied,
    /// The credentials were issued and the token endpoint refused the pair.
    ClientRejected,
    RefreshRefused,
    /// The provider could not be reached at all — not an answer, a failed call.
    Unreachable,
    /// The device endpoint answers something the plugin cannot read.
    DeviceUnreadable,
    /// The device endpoint refuses for a spent request budget.
    DeviceRateLimited,
}

/// The fields Real-Debrid's token endpoint reads, from the form body and nowhere else.
const TOKEN_FIELDS: [&str; 4] = ["client_id", "client_secret", "code", "grant_type"];
/// Real-Debrid's answer to a token request whose fields are not in a form body.
const PARAMETER_MISSING: &str = r#"{"error":"parameter_missing","error_code":1}"#;

/// One request the plugin made, flattened to what a test wants to assert on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    url: String,
    query: Vec<(String, String)>,
    /// The body's fields, read only when it was declared a form -- as the provider reads it.
    form: Vec<(String, String)>,
}

fn lookup<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

impl Recorded {
    /// A query parameter.
    fn field(&self, name: &str) -> Option<&str> {
        lookup(&self.query, name)
    }

    /// A field of the form body.
    fn form_field(&self, name: &str) -> Option<&str> {
        lookup(&self.form, name)
    }

    /// Whether the token endpoint would read this request: every field in the form body, none
    /// of them in the query.
    fn is_readable_token_request(&self) -> bool {
        TOKEN_FIELDS.iter().all(|name| {
            self.form_field(name).is_some_and(|value| !value.is_empty())
                && self.field(name).is_none()
        })
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

/// What `store-flow-secret` was called with.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Part {
    account_id: AccountId,
    name: String,
    value: String,
}

struct MockRealDebrid {
    case: Mutex<Case>,
    /// Whether the account already keeps the personal client of an earlier sign-in, the state
    /// a renewal runs in.
    client_kept: bool,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
    parts: Mutex<Vec<Part>>,
}

impl MockRealDebrid {
    fn new(case: Case) -> Arc<Self> {
        Self::build(case, false)
    }

    /// An account signed in before, keeping its personal client for the renewal.
    fn signed_in(case: Case) -> Arc<Self> {
        Self::build(case, true)
    }

    fn build(case: Case, client_kept: bool) -> Arc<Self> {
        Arc::new(Self {
            case: Mutex::new(case),
            client_kept,
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
            parts: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn stored(&self) -> Vec<Stored> {
        self.stored.lock().expect("stored").clone()
    }

    fn parts(&self) -> Vec<Part> {
        self.parts.lock().expect("parts").clone()
    }

    fn answer(&self, request: &Recorded) -> Result<HostHttpResponse, Failure> {
        let url = request.url.as_str();
        let case = *self.case.lock().expect("case");
        let body = |status: u16, text: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: url::Url::parse(url).expect("url"),
                headers,
                body: text.as_bytes().to_vec(),
            })
        };
        let retry_after = || {
            vec![ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }]
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
                Case::DeviceRateLimited => body(429, QUOTA, retry_after()),
                _ => body(200, DEVICE_CODE, Vec::new()),
            };
        }
        if url.starts_with(CREDENTIALS_ENDPOINT) {
            return match case {
                Case::Pending => body(400, PENDING, Vec::new()),
                Case::PendingUnnamed => body(403, CREDENTIALS_PENDING, Vec::new()),
                Case::SlowDown => body(429, QUOTA, retry_after()),
                Case::Expired => body(400, EXPIRY, Vec::new()),
                Case::Denied => body(403, REFUSED, Vec::new()),
                _ => body(200, CREDENTIALS, Vec::new()),
            };
        }
        // What Real-Debrid does with a token request whose fields are not in the form body.
        if !request.is_readable_token_request() {
            return body(400, PARAMETER_MISSING, Vec::new());
        }
        match case {
            Case::ClientRejected => body(401, CLIENT_REJECTED, Vec::new()),
            Case::RefreshRefused => body(401, REFRESH_REFUSED, Vec::new()),
            _ => body(200, SUCCESS, Vec::new()),
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
        let query = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        let is_form = request.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("content-type")
                && header.value_template == "application/x-www-form-urlencoded"
        });
        let form = if is_form {
            url::form_urlencoded::parse(&request.body)
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect()
        } else {
            Vec::new()
        };
        let recorded = Recorded {
            method: request.method.clone(),
            url: request.url.to_string(),
            query,
            form,
        };
        self.requests
            .lock()
            .expect("requests")
            .push(recorded.clone());
        self.answer(&recorded)
    }

    /// Whether a part exists — never what it is: the one question `secret-available` answers.
    async fn secret_available(&self, account_id: AccountId, reference: &str) -> bool {
        let is_client_part = matches!(
            reference,
            "realdebrid_client_id" | "realdebrid_client_secret"
        );
        (self.client_kept && is_client_part)
            || self
                .parts()
                .iter()
                .any(|part| part.account_id == account_id && part.name == reference)
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

    async fn store_flow_secret(
        &self,
        account_id: AccountId,
        name: &str,
        value: &str,
    ) -> Result<(), Failure> {
        self.parts.lock().expect("parts").push(Part {
            account_id,
            name: name.to_owned(),
            value: value.to_owned(),
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

/// The whole point of the job, in one test: a code confirmed on another screen ends in the
/// person's own client, a stored token *and* refresh material, so the renewal sweep keeps the
/// account alive and nobody is ever asked to type a second code — or to register anything.
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

    // The flow starts with the public client id and asks for a personal client; nothing had to
    // exist on the account first.
    let begin = server.requests().pop().expect("the device request");
    assert_eq!(begin.method, "GET");
    assert_eq!(begin.url, DEVICE_ENDPOINT);
    assert_eq!(begin.field("client_id"), Some(PUBLIC_CLIENT_ID));
    assert_eq!(begin.field("new_credentials"), Some("yes"));

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

    let requests = server.requests();
    let [_, claim, exchange] = requests.as_slice() else {
        panic!("device, credentials and token requests, got {requests:?}");
    };
    assert_eq!(claim.method, "GET");
    assert_eq!(claim.url, CREDENTIALS_ENDPOINT);
    assert_eq!(claim.field("client_id"), Some(PUBLIC_CLIENT_ID));
    assert_eq!(claim.field("code"), Some(PLACEHOLDER_DEVICE_CODE));

    // The personal client went to the host as two parts, each on its own and for this account.
    assert_eq!(
        server.parts(),
        vec![
            Part {
                account_id: account,
                name: "realdebrid_client_id".to_owned(),
                value: PLACEHOLDER_CLIENT_ID.to_owned(),
            },
            Part {
                account_id: account,
                name: "realdebrid_client_secret".to_owned(),
                value: PLACEHOLDER_CLIENT_SECRET.to_owned(),
            },
        ]
    );

    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, TOKEN_ENDPOINT);
    // Everything in the form body and nothing in the query: Real-Debrid reads no other shape.
    assert!(exchange.query.is_empty(), "{exchange:?}");
    assert!(exchange.is_readable_token_request(), "{exchange:?}");
    assert_eq!(exchange.form_field("grant_type"), Some(GRANT_TYPE));
    assert_eq!(exchange.form_field("code"), Some(PLACEHOLDER_DEVICE_CODE));
    // Named, never held: the exchange carries the markers the host expands, not the values the
    // plugin read a moment earlier.
    assert_eq!(
        exchange.form_field("client_id"),
        Some("{{secret:realdebrid_client_id}}")
    );
    assert_eq!(
        exchange.form_field("client_secret"),
        Some("{{secret:realdebrid_client_secret}}")
    );
    // A device grant carries no PKCE verifier: there is no authorization code to bind.
    assert!(exchange.form_field("code_verifier").is_none());

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
    // grant type, the personal client and the stored material, and the person asked nothing.
    let renewed = plugin
        .refresh(account, Some("account/renewal/reference"))
        .await
        .expect("refresh");
    assert_eq!(renewed, TokenOutcome::Authorized);
    let renewal = server.requests().pop().expect("a renewal request");
    assert_eq!(renewal.url, TOKEN_ENDPOINT);
    assert!(renewal.query.is_empty(), "{renewal:?}");
    assert_eq!(renewal.form_field("grant_type"), Some(GRANT_TYPE));
    assert_eq!(
        renewal.form_field("client_id"),
        Some("{{secret:realdebrid_client_id}}")
    );
    assert_eq!(
        renewal.form_field("client_secret"),
        Some("{{secret:realdebrid_client_secret}}")
    );
    // The material left the plugin as a template. The plugin never held the value, and the
    // host is what substitutes it on the way out.
    assert_eq!(
        renewal.form_field("code"),
        Some("{{secret:account/renewal/reference}}")
    );
    assert_eq!(server.stored().len(), 2);
    // No value the host keeps ever travelled in a request the plugin described.
    for request in server.requests() {
        for (_, value) in request.query.iter().chain(&request.form) {
            for kept in [
                PLACEHOLDER_CLIENT_SECRET,
                PLACEHOLDER_ACCESS_TOKEN,
                PLACEHOLDER_REFRESH_TOKEN,
            ] {
                assert!(!value.contains(kept), "{} carried {kept}", request.url);
            }
        }
    }
}

/// Waiting is not refusal. A sign-in that read "not yet" as an ending would die while the
/// person was still walking to the other screen — whether the provider says so in the RFC
/// word or in an error of its own.
#[tokio::test]
async fn an_unconfirmed_code_keeps_the_sign_in_open() {
    let bytes = component();
    for case in [Case::Pending, Case::PendingUnnamed] {
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
        assert!(
            matches!(
                outcome,
                TokenOutcome::Pending {
                    retry_after_seconds: 5
                }
            ),
            "{case:?}: {outcome:?}"
        );
        assert!(server.stored().is_empty(), "{case:?}");
        assert!(server.parts().is_empty(), "{case:?}");
        // Nothing was asked of the token endpoint before the code was confirmed.
        assert!(
            server
                .requests()
                .iter()
                .all(|request| request.url != TOKEN_ENDPOINT),
            "{case:?}"
        );
    }
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

/// Before any code exists, a spent budget cannot be waited out by the sweep — there is no flow
/// yet — so it is an error under its own code, carrying the provider's wait.
#[tokio::test]
async fn a_spent_budget_before_the_sign_in_starts_is_rate_limited() {
    let bytes = component();
    let server = MockRealDebrid::new(Case::DeviceRateLimited);
    let plugin = provider(server.clone(), &bytes);
    let failure = plugin
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("a spent budget is no device code");
    assert!(
        format!("{failure:#}").contains("fewer requests"),
        "{failure:#}"
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
    let server = MockRealDebrid::signed_in(Case::RefreshRefused);
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

/// A renewal with the personal client kept runs without the person: an expired access token is
/// replaced from the stored material alone.
#[tokio::test]
async fn an_expired_token_is_renewed_from_the_kept_client() {
    let bytes = component();
    let server = MockRealDebrid::signed_in(Case::Granted);
    let plugin = provider(server.clone(), &bytes);
    let account = AccountId::new();
    let outcome = plugin
        .refresh(account, Some("account/renewal/reference"))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        1,
        "one request and no device code: {requests:?}"
    );
    assert_eq!(requests[0].url, TOKEN_ENDPOINT);
    assert_eq!(server.stored().len(), 1);
    assert!(server.parts().is_empty(), "a renewal issues no new client");
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
    plugin
        .device_poll(AccountId::new(), Some(PLACEHOLDER_DEVICE_CODE))
        .await
        .expect_err("an unreachable provider is an error");
    assert!(server.stored().is_empty());
    assert!(server.parts().is_empty());
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
/// empty answer in it — whether the refresh material or the personal client is missing.
#[tokio::test]
async fn a_renewal_without_stored_material_never_reaches_the_provider() {
    let bytes = component();
    for (server, reference) in [
        (MockRealDebrid::signed_in(Case::Granted), None),
        (
            MockRealDebrid::new(Case::Granted),
            Some("account/renewal/reference"),
        ),
    ] {
        let plugin = provider(server.clone(), &bytes);
        let outcome = plugin
            .refresh(AccountId::new(), reference)
            .await
            .expect("refresh");
        assert!(
            matches!(outcome, TokenOutcome::Failed { .. }),
            "a renewal with nothing to renew should fail, got {outcome:?}"
        );
        assert!(server.requests().is_empty());
    }
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
        ("credentials.json", CREDENTIALS),
        ("credentials_pending.json", CREDENTIALS_PENDING),
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
            "client_id",
            "client_secret",
        ] {
            let Some(found) = value.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            assert!(
                [
                    PLACEHOLDER_ACCESS_TOKEN,
                    PLACEHOLDER_REFRESH_TOKEN,
                    PLACEHOLDER_DEVICE_CODE,
                    PLACEHOLDER_CLIENT_ID,
                    PLACEHOLDER_CLIENT_SECRET,
                ]
                .contains(&found),
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
    // No cookies, no captcha, no raw sockets.
    assert!(manifest.capabilities.net_stream.is_none());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    // The personal client the flow keeps, and nothing else: not the token the resolver sends,
    // not the API token a person may type in the other mode.
    assert_eq!(
        manifest.capabilities.secrets,
        [
            "realdebrid_client_id".to_owned(),
            "realdebrid_client_secret".to_owned()
        ]
    );
    // Device code and nothing else, so the host never asks for a redirect this provider has
    // no endpoint for.
    assert_eq!(
        manifest.oauth_flows(),
        [OAuthFlowManifest::Device].as_slice()
    );
    assert!(!manifest.serves_oauth_flow(OAuthFlowManifest::Redirect));
    // The Real-Debrid provider again (RD-150-09): its "Connect with a code" mode is this.
    let claims = manifest
        .extension
        .as_ref()
        .map(|extension| extension.claims.clone())
        .unwrap_or_default();
    assert_eq!(claims, ["realdebrid".to_owned()]);
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

/// The provider row offers the two ways in: a sign-in with a code by default, whose slots the
/// flow fills and nobody types, and the private API token beside it (RD-150-09).
#[test]
fn the_provider_offers_a_sign_in_with_a_code_and_the_api_token() {
    use rd_provider_registry::{CredentialKind, CredentialMode};

    const RESOLVER: &str = include_str!("../../../plugins/realdebrid/manifest.toml");
    let resolver: PluginManifest = toml::from_str(RESOLVER).expect("the resolver manifest");
    let row = rd_plugin_host::provider_spec_from_manifest(&resolver).expect("a provider row");
    let spec = row.spec;
    assert_eq!(spec.credentials, CredentialKind::OAuthOrApiKey);
    assert!(!spec.username_required);
    assert_eq!(
        spec.credential_modes(),
        vec![CredentialMode::OAuth, CredentialMode::ApiKey]
    );
    assert_eq!(spec.default_credential_mode(), Some(CredentialMode::OAuth));

    // What the person types, in the other mode only.
    let typed = spec.person_secret_slot().expect("what is typed");
    assert_eq!(typed.reference, "realdebrid_api_token");
    assert_eq!(typed.mode, Some(CredentialMode::ApiKey));

    // The token the sign-in obtains, and the two parts it keeps beside it.
    let token = spec.flow_secret_slot().expect("the token's slot");
    assert_eq!(token.reference, "realdebrid_access_token");
    for part in ["realdebrid_client_id", "realdebrid_client_secret"] {
        let slot = spec.flow_part_slot(part).expect("a part slot");
        assert_eq!(slot.mode, Some(CredentialMode::OAuth));
    }
    assert!(spec.flow_part_slot("realdebrid_access_token").is_none());
    assert!(spec.flow_part_slot("realdebrid_api_token").is_none());
    // Every one of them reaches the provider's own API and nothing else.
    assert!(
        spec.secrets
            .iter()
            .all(|slot| slot.domains == ["api.real-debrid.com".to_owned()])
    );
}

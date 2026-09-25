//! The resolver, driven against a mock of `www.seedr.cc/rest`.
//!
//! No socket is opened, no account is needed and no request leaves the machine: the mock
//! answers at the host boundary, so a test also sees each request exactly as the plugin
//! described it — which is what lets it assert that the account's password left the plugin as
//! the template `{{basic:seedr_password}}` and never as a value, and that neither half of a
//! HTTP Basic credential is anywhere in the request.
//!
//! **A run against the real provider is not claimed here.** It needs a premium Seedr account;
//! `docs/roadmap/jobs/120-04-seedr-feasibility.md` records that as open.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind, LinkStatus};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};

use super::SeedrResolver;

const AUTHORIZATION_TEMPLATE: &str = "Basic {{basic:seedr_password}}";
const FILE_URL: &str = "https://www.seedr.cc/rest/file/42";

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    has_secret: bool,
}

impl MockHost {
    fn new(responses: Vec<HostHttpResponse>, has_secret: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_secret,
        })
    }

    fn requests(&self) -> Vec<HostHttpRequest> {
        self.requests.lock().expect("mock lock").clone()
    }
}

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.requests.lock().expect("mock lock").push(request);
        self.responses
            .lock()
            .expect("mock lock")
            .pop_front()
            .ok_or_else(|| Failure::new(FailureKind::Permanent, "missing mock response"))
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_secret && reference == "seedr_password"
    }
}

fn answer(status: u16, body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://www.seedr.cc/rest/user".parse().expect("URL"),
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn resolver(host: &Arc<MockHost>) -> SeedrResolver {
    SeedrResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

fn client() -> ClientIdentity {
    ClientIdentity {
        account_id: Some(AccountId::new()),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// The whole point of the marker: the plugin names a vault reference and never holds either
/// half of the credential, so neither an address nor a password can appear in what it sent.
#[tokio::test]
async fn the_account_check_sends_the_basic_template_and_never_a_credential() {
    let host = MockHost::new(
        vec![answer(
            200,
            r#"{"username":"person@example.test","space_max":100,"space_used":40}"#,
        )],
        true,
    );
    let account = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("an account");
    assert!(account.valid);
    assert!(account.premium);

    let request = &host.requests()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.url.as_str(), "https://www.seedr.cc/rest/user");
    let authorization = request
        .headers
        .iter()
        .find(|header| header.name == "Authorization")
        .expect("an authorization header");
    assert_eq!(authorization.value_template, AUTHORIZATION_TEMPLATE);
    let sent = format!("{request:?}");
    assert!(!sent.contains("person@example.test"), "{sent}");
    assert!(!sent.to_ascii_lowercase().contains("password\":"), "{sent}");
}

/// An account with no password stored is told so by name, rather than being sent out with half
/// a credential and coming back as "Seedr rejected your sign-in".
#[tokio::test]
async fn an_account_without_a_password_is_refused_before_a_request_goes_out() {
    let host = MockHost::new(Vec::new(), false);
    let failure = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("seedr.password_missing"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn a_rejected_credential_ends_the_account() {
    let host = MockHost::new(vec![answer(401, "<html>401</html>")], true);
    let failure = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("seedr.auth_invalid"));
}

/// Seedr has no per-file metadata call, so a resolve reaches nothing and answers with the
/// canonical address — which is also what makes it survive a wait in the queue.
#[tokio::test]
async fn a_resolve_reaches_nothing_and_answers_with_the_stable_address() {
    let host = MockHost::new(Vec::new(), true);
    let resolved = resolver(&host)
        .resolve(ResolveRequest {
            url: format!("{FILE_URL}?download=1").parse().expect("URL"),
            client: client(),
        })
        .await
        .expect("a download");
    assert_eq!(resolved.url.as_str(), FILE_URL);
    // A resolver states download headers as values and this one has none to state: the engine
    // attaches the account's Basic pair to the bytes itself (`transfer_auth = "basic"`).
    assert!(resolved.headers.is_empty());
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn an_address_that_is_not_a_seedr_file_is_refused() {
    let host = MockHost::new(Vec::new(), true);
    let failure = resolver(&host)
        .resolve(ResolveRequest {
            url: "https://www.seedr.cc/rest/folder/42".parse().expect("URL"),
            client: client(),
        })
        .await
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("seedr.not_a_seedr_link"));
}

/// `unknown` rather than `online`: reporting a status without asking would show a green row
/// for a file somebody deleted last week, and Seedr has no call that answers the question.
#[tokio::test]
async fn a_link_check_says_it_does_not_know_rather_than_guessing() {
    let host = MockHost::new(Vec::new(), true);
    let results = resolver(&host)
        .check(CheckRequest {
            urls: vec![FILE_URL.parse().expect("URL")],
            client: client(),
        })
        .await
        .expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, LinkStatus::Unknown);
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn the_account_catalogue_names_seedr_and_nobody_else() {
    let host = MockHost::new(Vec::new(), true);
    let hosters = resolver(&host)
        .hosters(AccountId::new())
        .await
        .expect("a catalogue");
    assert_eq!(hosters, vec!["seedr.cc".to_owned()]);
}

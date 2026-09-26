//! The Put.io resolver and sign-in, driven end to end against a mock of the provider
//! (RD-120-03).
//!
//! The two siblings of `putio_remote_job_contract.rs`, in one file because they share one
//! account, one provider row and one mock. Both run as real WebAssembly components -- built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-putio
//! -p rd-plugin-putio-oauth` -- and the mock answers at the host boundary, so no socket is
//! opened, no account is needed and no request leaves the machine.
//!
//! What the resolver half proves is mostly about what it does *not* do:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A file address is resolved | the stable per-file address, its name, size and checksum |
//! | `files/{id}/url` | never requested -- rDownloader holds no expiring address |
//! | The token | leaves as `{{secret:putio_access_token}}`, never as a value |
//! | A folder address | refused, because a folder has no bytes |
//! | The sign-in expired | `putio.auth_invalid`, an account to fix and not a link to retry |
//! | The file is gone | `putio.file_not_found`; a link check says offline |
//! | The budget is spent | a wait carrying the window Put.io named |
//!
//! And the sign-in half:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | The address the person is sent to | on `api.put.io`, carrying `state` and the client marker |
//! | The person agreed | `Authorized`, the token stored, no expiry and no refresh material |
//! | The client secret | leaves as `{{secret:putio_client_secret}}`, never as a value |
//! | Put.io refused | `Failed`, under a code the catalogue translates |
//! | A device code was asked for | refused: Put.io's out-of-band entrance is not implemented |
//!
//! **A run against the real provider is not claimed here.** It needs a Put.io account;
//! `docs/roadmap/jobs/archive/120-03-putio.md` records that as open.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind, LinkStatus};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest,
    ResolvedHeader, Resolver, ResolverHost,
};
use rd_plugin_host::{
    ComponentResolver, PluginManifest,
    extension::{OAuthProvider, TokenOutcome},
};
use url::Url;

/// What the mock host's clock always reads (2026-09-25), so a wait worked out from Put.io's
/// absolute `X-RateLimit-Reset` is exact rather than a wall-clock race (RD-120-67).
const MOCK_NOW: u64 = 1_790_294_400;
const RESOLVER_MANIFEST: &str = include_str!("../../../plugins/putio/manifest.toml");
const OAUTH_MANIFEST: &str = include_str!("../../../plugins/putio-oauth/manifest.toml");

const ACCOUNT_INFO: &str = include_str!("fixtures/putio/account_info.json");
const FILE: &str = include_str!("fixtures/putio/file.json");
const FOLDER: &str = include_str!("fixtures/putio/folder.json");
const ERROR_TOKEN: &str = include_str!("fixtures/putio/error_invalid_token.json");
const ERROR_NOT_FOUND: &str = include_str!("fixtures/putio/error_not_found.json");
const ERROR_TOO_MANY: &str = include_str!("fixtures/putio/error_too_many_requests.json");
const OAUTH_GRANTED: &str = include_str!("fixtures/putio/oauth_granted.json");
const OAUTH_REFUSED: &str = include_str!("fixtures/putio/oauth_refused.json");

const FILE_ADDRESS: &str = "https://api.put.io/v2/files/900002/download";
const FOLDER_ADDRESS: &str = "https://api.put.io/v2/files/900001";
const TOKEN_TEMPLATE: &str = "Bearer {{secret:putio_access_token}}";
const SECRET_TEMPLATE: &str = "{{secret:putio_client_secret}}";
const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";

/// What the mock should answer next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    Serving,
    ServingFolder,
    TokenExpired,
    NotFound,
    RateLimited,
    /// The token endpoint hands the token over.
    Granted,
    /// The token endpoint says no.
    Refused,
}

/// One request the plugin made, flattened to what a test asserts on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    authorization: Option<String>,
}

/// What `store-oauth-token` was called with.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Stored {
    access_token: String,
    refresh_token: Option<String>,
    expires_in_seconds: Option<u64>,
}

struct MockPutio {
    case: Case,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl MockPutio {
    fn new(case: Case) -> Arc<Self> {
        Arc::new(Self {
            case,
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
impl ResolverHost for MockPutio {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let path = request.url.path().to_owned();
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            path: path.clone(),
            query: request
                .query
                .iter()
                .map(|value| (value.name.clone(), value.value_template.clone()))
                .collect(),
            authorization: request
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("authorization"))
                .map(|header| header.value_template.clone()),
        });
        let answer = |status: u16, body: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers,
                body: body.as_bytes().to_vec(),
            })
        };
        match self.case {
            Case::TokenExpired => answer(401, ERROR_TOKEN, Vec::new()),
            Case::NotFound => answer(404, ERROR_NOT_FOUND, Vec::new()),
            Case::RateLimited => answer(
                429,
                ERROR_TOO_MANY,
                vec![ResolvedHeader {
                    name: "X-RateLimit-Reset".to_owned(),
                    // Five minutes after the mock host's clock, so the wait the plugin works
                    // out with that clock is a real subtraction rather than a constant.
                    value: (MOCK_NOW + 300).to_string(),
                }],
            ),
            Case::Granted => answer(200, OAUTH_GRANTED, Vec::new()),
            Case::Refused => answer(400, OAUTH_REFUSED, Vec::new()),
            Case::ServingFolder => answer(200, FOLDER, Vec::new()),
            Case::Serving if path.ends_with("/account/info") => {
                answer(200, ACCOUNT_INFO, Vec::new())
            }
            Case::Serving if path == "/v2/files/900001" => answer(200, FOLDER, Vec::new()),
            Case::Serving if path.starts_with("/v2/files/") => answer(200, FILE, Vec::new()),
            Case::Serving => answer(404, ERROR_NOT_FOUND, Vec::new()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "putio_access_token"
    }

    fn now_unix_seconds(&self) -> u64 {
        MOCK_NOW
    }

    async fn store_oauth_token(
        &self,
        _account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        self.stored.lock().expect("stored").push(Stored {
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.map(str::to_owned),
            expires_in_seconds,
        });
        Ok(())
    }
}

fn resolver_manifest() -> PluginManifest {
    toml::from_str(RESOLVER_MANIFEST).expect("the resolver manifest")
}

fn oauth_manifest() -> PluginManifest {
    toml::from_str(OAUTH_MANIFEST).expect("the sign-in manifest")
}

fn resolver(host: Arc<MockPutio>) -> ComponentResolver {
    let component = rd_plugin_host::artifact::component("rd-plugin-putio");
    ComponentResolver::new(resolver_manifest(), &component, host).expect("the component loads")
}

fn sign_in(host: Arc<MockPutio>) -> OAuthProvider {
    let component = rd_plugin_host::artifact::component("rd-plugin-putio-oauth");
    OAuthProvider::new(oauth_manifest(), &component, Some(host)).expect("the component loads")
}

fn request(url: &str, account: AccountId) -> ResolveRequest {
    ResolveRequest {
        url: Url::parse(url).expect("a URL"),
        client: ClientIdentity {
            account_id: Some(account),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

/// Routing happens in two steps and this checks both. The host asks `matches` first, which is a
/// question about the *host name* and is answered from the manifest's `match_domains`; the guest
/// then decides whether the address is actually one of its own. Put.io is not a multihoster, so
/// the first step already refuses everybody else's hosts.
#[tokio::test]
async fn only_put_ios_own_hosts_reach_this_plugin_at_all() {
    let resolver = resolver(MockPutio::new(Case::Serving));
    for claimed in [
        FILE_ADDRESS,
        "https://api.put.io/v2/files/900002",
        "https://app.put.io/files/900002",
    ] {
        assert!(
            resolver.matches(&Url::parse(claimed).expect("a URL")),
            "{claimed}"
        );
    }
    for foreign in [
        // A hoster this plugin does not unrestrict: Put.io is not a multihoster, and a
        // `match_domains` of `*` is what a multihoster declares.
        "https://ddownload.com/f/abc",
        // Somebody else's host, spelled to look like Put.io's.
        "https://api.put.io.example.invalid/v2/files/1",
        "https://putio.example.invalid/v2/files/1",
    ] {
        assert!(
            !resolver.matches(&Url::parse(foreign).expect("a URL")),
            "{foreign}"
        );
    }
}

/// The second step: an address on Put.io's own host that names no file is not this plugin's.
///
/// The guest answers that from the address alone, and the host acts on it before a resolve ever
/// starts -- `plugin.url_rejected` is the host saying the plugin's own `match_url` said no. That
/// is the assertion worth making: a plugin that claimed a folder listing would take it away from
/// whatever could have handled it, and would then fail on it.
#[tokio::test]
async fn an_address_on_put_ios_host_that_names_no_file_is_not_claimed() {
    let host = MockPutio::new(Case::Serving);
    let resolver = resolver(Arc::clone(&host));
    for foreign in [
        // A folder listing is not a file.
        "https://app.put.io/files/900001/children",
        // An id that is not a number, including the shape that would leave the path.
        "https://api.put.io/v2/files/../account/info",
        "https://api.put.io/v2/files/abc",
        // Put.io's own account endpoint is not a download.
        "https://api.put.io/v2/account/info",
    ] {
        let failure = resolver
            .resolve(request(foreign, AccountId::new()))
            .await
            .expect_err("refused");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.url_rejected"),
            "{foreign}"
        );
    }
    assert!(
        host.requests().is_empty(),
        "deciding what an address is reaches nothing"
    );
}

/// One resolve: one request, the stable address back, and no short-lived one anywhere.
///
/// The last assertion is the point of the whole design. `GET /v2/files/{id}/url` exists and
/// would answer with a signed address that expires; a queue that waits an hour would then hold
/// a refusal nobody can act on. It is never asked for.
#[tokio::test]
async fn a_file_resolves_to_the_stable_address_and_never_to_an_expiring_one() {
    let host = MockPutio::new(Case::Serving);
    let resolver = resolver(Arc::clone(&host));
    let account = AccountId::new();
    let resolved = resolver
        .resolve(request(FILE_ADDRESS, account))
        .await
        .expect("resolved");

    assert_eq!(resolved.url.as_str(), FILE_ADDRESS);
    assert_eq!(resolved.file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(resolved.size.map(rd_core::ByteCount::get), Some(10));
    // No headers: the credential this address needs is the account's own token, and the host
    // attaches that because the `putio` provider row says it may. A resolver states headers as
    // values, and it has no value to state.
    assert!(resolved.headers.is_empty());
    let checksum = resolved.checksum.expect("a checksum");
    assert_eq!(checksum.algorithm, rd_core::ChecksumAlgorithm::Crc32);
    assert_eq!(checksum.value, "1a2b3c4d");

    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/v2/files/900002");
    assert!(
        requests
            .iter()
            .all(|request| !request.path.ends_with("/url")),
        "a short-lived address is never asked for"
    );
}

/// The token the plugin sends is the vault template, on every request, and never a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let host = MockPutio::new(Case::Serving);
    let resolver = resolver(Arc::clone(&host));
    let account = AccountId::new();
    let _ = resolver.resolve(request(FILE_ADDRESS, account)).await;
    let _ = resolver.check_account(account).await;
    let authorizations: Vec<String> = host
        .requests()
        .into_iter()
        .filter_map(|request| request.authorization)
        .collect();
    assert_eq!(authorizations.len(), 2);
    assert!(
        authorizations.iter().all(|value| value == TOKEN_TEMPLATE),
        "{authorizations:?}"
    );
}

/// A folder has no bytes. Said plainly rather than as "this file is empty", which is what a
/// folder's record looks like from here.
#[tokio::test]
async fn a_folder_address_is_refused_for_what_it_is() {
    let resolver = resolver(MockPutio::new(Case::ServingFolder));
    let failure = resolver
        .resolve(request(FOLDER_ADDRESS, AccountId::new()))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("putio.is_a_folder"));
    assert_eq!(failure.category, FailureKind::Unsupported);
}

/// The account, as the accounts list shows it: valid, paying, named, with its free space as a
/// translated part rather than as remaining traffic it is not.
#[tokio::test]
async fn the_account_check_reports_the_account_and_not_a_number_it_is_not() {
    let resolver = resolver(MockPutio::new(Case::Serving));
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("checked");
    assert!(status.valid);
    assert!(status.premium, "put.io has no free tier");
    assert_eq!(
        status.traffic_left, None,
        "free storage is not traffic left"
    );
    let codes: Vec<&str> = status.label.iter().map(|part| part.code.as_str()).collect();
    assert_eq!(codes, vec!["plugin.account.user", "putio.disk_free"]);
    let free = status
        .label
        .iter()
        .find(|part| part.code == "putio.disk_free")
        .expect("the free-space part");
    assert_eq!(free.params.get("bytes").map(String::as_str), Some("1024"));
}

/// An expired sign-in is an account to fix, not a link to retry. The two are different things
/// to the scheduler and different sentences to the person.
#[tokio::test]
async fn an_expired_sign_in_is_reported_as_an_account_to_fix() {
    let resolver = resolver(MockPutio::new(Case::TokenExpired));
    let failure = resolver
        .resolve(request(FILE_ADDRESS, AccountId::new()))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("putio.auth_invalid"));
    assert_eq!(failure.category, FailureKind::AccountInvalid);
    // The stable word travels; Put.io's own sentence does not.
    assert!(
        failure
            .params
            .iter()
            .any(|(name, value)| name == "reason" && value == "INVALID_TOKEN"),
        "{:?}",
        failure.params
    );
    assert!(!failure.message.contains("redacted provider sentence"));
}

/// A file Put.io no longer holds is gone, and a link check says so rather than leaving it
/// unknown -- which is the difference between a row a person can clear and a row that sits
/// there being retried.
#[tokio::test]
async fn a_file_put_io_no_longer_holds_is_offline() {
    let resolver = resolver(MockPutio::new(Case::NotFound));
    let account = AccountId::new();
    let failure = resolver
        .resolve(request(FILE_ADDRESS, account))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("putio.file_not_found"));

    let checked = resolver
        .check(CheckRequest {
            urls: vec![Url::parse(FILE_ADDRESS).expect("a URL")],
            client: ClientIdentity {
                account_id: Some(account),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("checked");
    assert_eq!(checked.len(), 1);
    assert_eq!(checked[0].status, LinkStatus::Offline);
}

/// A spent request budget is a wait, and the wait is the window Put.io named: its
/// `X-RateLimit-Reset` is an absolute timestamp, so the plugin turns it into a duration with
/// the host's own clock rather than guessing.
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_put_ios_own_window() {
    let resolver = resolver(MockPutio::new(Case::RateLimited));
    let failure = resolver
        .resolve(request(FILE_ADDRESS, AccountId::new()))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("putio.rate_limited"));
    let FailureKind::RateLimited {
        retry_after_seconds: Some(waited),
    } = failure.category
    else {
        panic!(
            "expected a rate limit carrying a wait, got {:?}",
            failure.category
        );
    };
    assert_eq!(waited, 300);
}

/// Where the person is sent: Put.io's own authorization endpoint, with an unguessable `state`
/// and the marker the host replaces with this installation's client id. No `code_challenge` --
/// Put.io's exchange authenticates with the client secret and publishes no PKCE support, and a
/// proof nobody checks is decoration.
#[tokio::test]
async fn the_sign_in_sends_the_person_to_put_io_with_an_unguessable_state() {
    let sign_in = sign_in(MockPutio::new(Case::Granted));
    let first = sign_in
        .begin(AccountId::new(), None)
        .await
        .expect("an authorization request");
    let address = Url::parse(&first.authorization_url).expect("a URL");
    assert_eq!(address.host_str(), Some("api.put.io"));
    assert_eq!(address.path(), "/v2/oauth2/authenticate");
    let parameter = |name: &str| {
        address
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    assert_eq!(parameter("client_id").as_deref(), Some("{{client_id}}"));
    assert_eq!(parameter("response_type").as_deref(), Some("code"));
    assert_eq!(parameter("state").as_deref(), Some(first.state.as_str()));
    assert_eq!(parameter("code_challenge"), None);
    assert!(first.flow_state.is_none(), "there is no verifier to keep");
    assert!(first.state.len() >= 43, "{}", first.state);

    // And it is drawn fresh: two sign-ins that shared a `state` would be two sign-ins either
    // callback could finish.
    let second = sign_in
        .begin(AccountId::new(), None)
        .await
        .expect("an authorization request");
    assert_ne!(first.state, second.state);
}

/// The exchange: the token is stored before `Authorized` is reported, the client secret leaves
/// as a template, and neither an expiry nor refresh material is invented for a provider that
/// states none.
#[tokio::test]
async fn a_granted_exchange_stores_the_token_and_invents_no_expiry() {
    let host = MockPutio::new(Case::Granted);
    let sign_in = sign_in(Arc::clone(&host));
    let outcome = sign_in
        .poll(AccountId::new(), "redacted-code-0000", None)
        .await
        .expect("exchanged");
    assert_eq!(outcome, TokenOutcome::Authorized);
    assert_eq!(
        host.stored(),
        vec![Stored {
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            // Put.io issues neither, and a plugin that invented an expiry would have the
            // renewal sweep asking a provider that has nothing to answer with.
            refresh_token: None,
            expires_in_seconds: None,
        }]
    );
    let exchange = host.requests().pop().expect("the exchange");
    assert_eq!(exchange.path, "/v2/oauth2/access_token");
    let field = |name: &str| {
        exchange
            .query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(field("client_id").as_deref(), Some("{{client_id}}"));
    assert_eq!(field("client_secret").as_deref(), Some(SECRET_TEMPLATE));
    assert_eq!(field("grant_type").as_deref(), Some("authorization_code"));
    assert_eq!(field("code").as_deref(), Some("redacted-code-0000"));
}

/// A refusal ends the sign-in under a code the catalogue translates, and nothing is stored.
#[tokio::test]
async fn a_refused_exchange_ends_the_sign_in_and_stores_nothing() {
    let host = MockPutio::new(Case::Refused);
    let sign_in = sign_in(Arc::clone(&host));
    let outcome = sign_in
        .poll(AccountId::new(), "redacted-code-0000", None)
        .await
        .expect("answered");
    let TokenOutcome::Failed { category, message } = outcome else {
        panic!("expected a refusal");
    };
    // The host carries the category and the redaction-safe text; the stable code the
    // catalogue translates is `putio_oauth.code_expired`, which `putio-oauth`'s own unit
    // tests pin to this very word.
    assert_eq!(category, FailureKind::AuthRequired);
    assert!(message.contains("INVALID_CODE"), "{message}");
    assert!(!message.contains("redacted provider sentence"), "{message}");
    assert!(host.stored().is_empty());
}

/// Put.io states no expiry, so the host's renewal sweep never reaches `refresh`. An account
/// with nothing to renew is told so once instead of being retried for ever, and nothing is
/// asked of the provider.
#[tokio::test]
async fn a_renewal_without_stored_material_ends_rather_than_asking_put_io() {
    let host = MockPutio::new(Case::Granted);
    let sign_in = sign_in(Arc::clone(&host));
    let outcome = sign_in
        .refresh(AccountId::new(), None)
        .await
        .expect("answered");
    let TokenOutcome::Failed { category, message } = outcome else {
        panic!("expected a refusal");
    };
    assert_eq!(category, FailureKind::AuthRequired);
    assert!(message.contains("no stored sign-in to renew"), "{message}");
    assert!(host.requests().is_empty());
}

/// The out-of-band entrance is not implemented, and being asked for it says so with a stable
/// code rather than a trap. `oauth_flows = ["redirect"]` means the host never asks.
#[tokio::test]
async fn the_device_entrance_refuses_with_a_code_the_catalogue_translates() {
    let host = MockPutio::new(Case::Granted);
    let sign_in = sign_in(Arc::clone(&host));
    let account = AccountId::new();
    let failure = sign_in
        .device_begin(account, None)
        .await
        .expect_err("refused");
    assert!(
        format!("{failure:?}").contains("putio_oauth.flow_unsupported"),
        "{failure:?}"
    );
    let failure = sign_in
        .device_poll(account, None)
        .await
        .expect_err("refused");
    assert!(
        format!("{failure:?}").contains("putio_oauth.flow_unsupported"),
        "{failure:?}"
    );
    assert!(host.requests().is_empty());
    assert_eq!(
        oauth_manifest().oauth_flows.len(),
        1,
        "the manifest offers exactly one entrance"
    );
}

/// The provider row every Put.io plugin hangs off, and the two credential slots it owns. The
/// second slot's domains are what let the host attach the account's token to the transfer
/// itself, which is what makes the stable address downloadable at all.
#[test]
fn the_provider_row_carries_the_two_slots_a_put_io_account_needs() {
    let manifest = resolver_manifest();
    let provider = manifest.provider.as_ref().expect("a provider section");
    assert_eq!(provider.slug, "putio");
    assert!(
        provider.username_required,
        "the client id is the account's username; a sign-in cannot start without one"
    );
    let slots = provider.secret_slots();
    let references: Vec<&str> = slots.iter().map(|slot| slot.reference.as_str()).collect();
    assert_eq!(
        references,
        vec!["putio_client_secret", "putio_access_token"]
    );
    assert!(
        slots
            .iter()
            .all(|slot| slot.domains == vec!["api.put.io".to_owned()]),
        "{slots:?}"
    );
    // The resolver reaches only the API, with only the token: the client secret is the
    // sign-in's business and a grant this plugin does not need is one it should not have.
    assert_eq!(
        manifest.capabilities.domains(),
        ["api.put.io".to_owned()].as_slice()
    );
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["putio_access_token".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
}

/// The sign-in reaches only the token endpoint's host, with only the client secret. The token
/// it obtains goes into the vault and is never read back, so it is not a grant this plugin has.
#[test]
fn the_sign_in_reaches_only_what_an_exchange_needs() {
    let manifest = oauth_manifest();
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["putio".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["api.put.io".to_owned()].as_slice()
    );
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["putio_client_secret".to_owned()]
    );
}

/// Nothing that could be a credential or a provider's own sentence about somebody's data is
/// committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/putio");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        walk(&value, &path);
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} fixtures were checked");

    fn walk(value: &serde_json::Value, path: &std::path::Path) {
        match value {
            serde_json::Value::Object(fields) => {
                for (name, inner) in fields {
                    // A token-shaped field may only ever hold the placeholder: this directory
                    // holds a granted exchange, which is the one document that has to carry a
                    // token at all.
                    if matches!(
                        name.as_str(),
                        "access_token" | "oauth_token" | "refresh_token" | "client_secret"
                    ) {
                        assert_eq!(
                            inner.as_str(),
                            Some(PLACEHOLDER_ACCESS_TOKEN),
                            "{path:?} carries a real `{name}`"
                        );
                    }
                    if name == "error_message" {
                        assert_eq!(
                            inner.as_str(),
                            Some("redacted provider sentence"),
                            "{path:?} carries a real provider sentence"
                        );
                    }
                    walk(inner, path);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, path);
                }
            }
            serde_json::Value::String(text) if text.starts_with("http") => {
                assert!(
                    text.contains("invalid") || text.starts_with("https://api.put.io/"),
                    "{path:?} carries a live link: {text}"
                );
            }
            _ => {}
        }
    }
}

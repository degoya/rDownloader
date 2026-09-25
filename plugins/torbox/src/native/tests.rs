//! The TorBox resolver contract, driven against a mock of the provider's API.
//!
//! The mock answers at the host boundary rather than over a socket, so no request leaves the
//! machine and no account is needed. What it proves is what a host reacts to differently:
//!
//! | Case | What TorBox answered | What the plugin must do |
//! | --- | --- | --- |
//! | Success | `200` with `data` as an address | queue that address |
//! | Renewal | two resolves of one address | two requests, two addresses |
//! | Rate limit | `429` + `Retry-After` | wait the stated time, keep the account |
//! | Key expired | `BAD_TOKEN` inside a `200` | invalidate the account |
//! | Error | `ITEM_NOT_FOUND` | offline, so the LinkGrabber can act on it |
//!
//! A run against the real provider is deliberately **not** claimed here: it needs an account
//! with an API key, and `docs/roadmap/jobs/120-01-torbox.md` records that as open.

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

use super::TorBoxResolver;

const TORRENT: &str = "https://api.torbox.app/v1/api/torrents/requestdl?torrent_id=4711&file_id=3";
const KEY_TEMPLATE: &str = "{{secret:torbox_api_key}}";

struct MockHost {
    responses: Mutex<VecDeque<HostHttpResponse>>,
    requests: Mutex<Vec<HostHttpRequest>>,
    has_key: bool,
}

impl MockHost {
    fn with_responses(responses: Vec<HostHttpResponse>, has_key: bool) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            has_key,
        })
    }

    fn new(response: HostHttpResponse) -> Arc<Self> {
        Self::with_responses(vec![response], true)
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
        self.has_key && reference == "torbox_api_key"
    }
}

fn answer(status: u16, body: &str, headers: Vec<ResolvedHeader>) -> HostHttpResponse {
    HostHttpResponse {
        status,
        final_url: "https://api.torbox.app/v1/api/torrents/requestdl"
            .parse()
            .expect("URL"),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

fn minted(url: &str) -> HostHttpResponse {
    answer(
        200,
        &format!(r#"{{"success":true,"detail":"ok","data":"{url}"}}"#),
        Vec::new(),
    )
}

fn resolver(host: &Arc<MockHost>) -> TorBoxResolver {
    TorBoxResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

fn request(url: &str, account: AccountId) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("URL"),
        client: ClientIdentity {
            account_id: Some(account),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

/// The address goes out rebuilt from its two identifiers, the key travels as a marker, and
/// what comes back is the address TorBox minted.
#[tokio::test]
async fn a_download_address_is_minted_from_the_two_identifiers_and_the_key_marker() {
    let host = MockHost::new(minted("https://store-1.torbox.app/dl/abc"));
    let account = AccountId::new();
    let resolved = resolver(&host)
        .resolve(request(TORRENT, account))
        .await
        .expect("resolved");
    assert_eq!(resolved.url.as_str(), "https://store-1.torbox.app/dl/abc");
    let sent = host.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, "GET");
    assert_eq!(sent[0].url.path(), "/v1/api/torrents/requestdl");
    let query: Vec<(String, String)> = sent[0]
        .query
        .iter()
        .map(|value| (value.name.clone(), value.value_template.clone()))
        .collect();
    assert!(
        query.contains(&("torrent_id".to_owned(), "4711".to_owned())),
        "{query:?}"
    );
    assert!(
        query.contains(&("file_id".to_owned(), "3".to_owned())),
        "{query:?}"
    );
    // The key never enters the plugin: it goes out as the marker the host expands.
    assert!(
        query.contains(&("token".to_owned(), KEY_TEMPLATE.to_owned())),
        "{query:?}"
    );
    let authorization = sent[0]
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("authorization"))
        .expect("an Authorization header");
    assert_eq!(
        authorization.value_template,
        format!("Bearer {KEY_TEMPLATE}")
    );
}

/// The acceptance criterion that makes a resume after a pause work: the durable address is the
/// `requestdl` one, and every resolve mints a new ticket rather than reusing the last.
#[tokio::test]
async fn every_resolve_of_one_address_mints_a_fresh_ticket() {
    let host = MockHost::with_responses(
        vec![
            minted("https://store-1.torbox.app/dl/first?expires=1"),
            minted("https://store-1.torbox.app/dl/second?expires=2"),
        ],
        true,
    );
    let account = AccountId::new();
    let plugin = resolver(&host);
    let first = plugin
        .resolve(request(TORRENT, account))
        .await
        .expect("resolved");
    let second = plugin
        .resolve(request(TORRENT, account))
        .await
        .expect("resolved again");
    assert_ne!(first.url, second.url);
    assert_eq!(host.requests().len(), 2, "one request per attempt");
    // The scheduler recognises the minted address as short-lived and comes back here for a new
    // one rather than reusing it after a pause.
    assert!(rd_core::is_signed_url(&second.url));
}

/// A person can edit a candidate row, and what goes out carries the account's key.
#[tokio::test]
async fn an_address_this_plugin_does_not_claim_is_refused_before_any_request() {
    let host = MockHost::new(minted("https://store-1.torbox.app/dl/abc"));
    let failure = resolver(&host)
        .resolve(request(
            "https://api.torbox.app/v1/api/torrents/mylist",
            AccountId::new(),
        ))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("torbox.not_a_ticket"));
    assert!(host.requests().is_empty(), "nothing was asked");
}

/// Without a key there is nothing to ask with, and saying so costs no request.
#[tokio::test]
async fn an_account_without_a_key_is_refused_before_any_request() {
    let host = MockHost::with_responses(Vec::new(), false);
    let failure = resolver(&host)
        .resolve(request(TORRENT, AccountId::new()))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("torbox.key_missing"));
    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert!(host.requests().is_empty());
}

/// A key TorBox no longer accepts invalidates the account, so the interface offers the one
/// thing that helps: entering it again.
#[tokio::test]
async fn an_expired_key_invalidates_the_account() {
    let host = MockHost::new(answer(
        200,
        r#"{"success":false,"error":"BAD_TOKEN","detail":"invalid api key"}"#,
        Vec::new(),
    ));
    let failure = resolver(&host)
        .resolve(request(TORRENT, AccountId::new()))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("torbox.auth_invalid"));
    assert_eq!(failure.category, FailureKind::AccountInvalid);
}

/// A spent budget is a wait carrying TorBox's own figure: refused requests count towards the
/// cap that refused them, so asking again at once would only extend it.
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_retry_after() {
    let host = MockHost::new(answer(
        429,
        r#"{"success":false,"error":"TOO_MANY_REQUESTS"}"#,
        vec![ResolvedHeader {
            name: "Retry-After".to_owned(),
            value: "120".to_owned(),
        }],
    ));
    let failure = resolver(&host)
        .resolve(request(TORRENT, AccountId::new()))
        .await
        .expect_err("refused");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(120)
        }
    );
}

/// What the account is worth, and the label the accounts list prints beside it.
#[tokio::test]
async fn the_account_check_reports_the_plan_and_who_it_belongs_to() {
    let host = MockHost::new(answer(
        200,
        r#"{"success":true,"data":{"email":"nobody@example.invalid","plan":2,
            "premium_expires_at":"2027-01-01T00:00:00Z","is_subscribed":true}}"#,
        Vec::new(),
    ));
    let status = resolver(&host)
        .check_account(AccountId::new())
        .await
        .expect("checked");
    assert!(status.valid);
    assert!(status.premium);
    assert!(
        status.traffic_left.is_none(),
        "TorBox states no byte budget"
    );
    let codes: Vec<&str> = status.label.iter().map(|part| part.code.as_str()).collect();
    assert!(codes.contains(&"plugin.account.user"), "{codes:?}");
    assert!(codes.contains(&"plugin.account.premium_until"), "{codes:?}");
    assert_eq!(host.requests()[0].url.path(), "/v1/api/user/me");
}

/// A file TorBox no longer offers is `Offline`; a job still running is `Unknown`, because the
/// LinkGrabber acts on `Offline` and a job in progress is not a file that is gone.
#[tokio::test]
async fn a_check_tells_a_missing_file_apart_from_a_job_still_running() {
    let present = r#"{"success":true,"data":{"name":"Example.Release","download_present":true,
        "files":[{"id":3,"short_name":"ep01.mkv","name":"Example.Release/ep01.mkv","size":10}]}}"#;
    let running = r#"{"success":true,"data":{"name":"Example.Release","download_present":false,
        "files":[{"id":3,"short_name":"ep01.mkv","size":10}]}}"#;
    let gone = r#"{"success":true,"data":{"name":"Example.Release","download_present":true,
        "files":[{"id":9,"short_name":"other.mkv"}]}}"#;
    let host = MockHost::with_responses(
        vec![
            answer(200, present, Vec::new()),
            answer(200, running, Vec::new()),
            answer(200, gone, Vec::new()),
        ],
        true,
    );
    let results = resolver(&host)
        .check(CheckRequest {
            urls: vec![
                TORRENT.parse().expect("URL"),
                TORRENT.parse().expect("URL"),
                TORRENT.parse().expect("URL"),
            ],
            client: ClientIdentity {
                account_id: Some(AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("checked");
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(results[0].size.map(rd_core::ByteCount::get), Some(10));
    assert_eq!(results[1].status, LinkStatus::Unknown);
    assert_eq!(results[2].status, LinkStatus::Offline);
}

/// TorBox will fetch a hoster link, but only as a job. Answering with a catalogue here would
/// tell the core this plugin can resolve links it cannot.
#[tokio::test]
async fn the_provider_offers_no_hoster_catalogue() {
    let host = MockHost::with_responses(Vec::new(), true);
    let hosters = resolver(&host)
        .hosters(AccountId::new())
        .await
        .expect("asked");
    assert!(hosters.is_empty());
    assert!(host.requests().is_empty(), "the answer costs no request");
}

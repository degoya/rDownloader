//! Native-adapter coverage for Box, driven against a mock Box Content API.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! api.box.com is not contacted. A run against a real Box account is *not* claimed here; there
//! is none in this checkout, and no registered Box application either. What is proven is
//! everything that does not need one: which address turns into which request, what each of
//! Box's refusals becomes, that a shared link's password reaches the `boxapi` header and
//! nowhere else, that the download address names the version it is the bytes of, and that the
//! account's token leaves this plugin as a marker rather than as a value.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};

use super::BoxResolver;

const OWN_FILE: &str = "https://app.box.com/file/123456789";
const SHARED_FILE: &str = "https://app.box.com/s/abc123def456/file/42";
const SHARED_FILE_WITH_PASSWORD: &str =
    "https://app.box.com/s/abc123def456/file/42?shared_link_password=hunter2";

/// The file document Box answers `/2.0/files/<id>` with, sanitised: real ids and digests
/// replaced, every field Box actually sends kept.
const FILE: &str = r#"{"type":"file","id":"123456789","etag":"3","name":"release.bin",
    "size":1048576,"sha1":"aabbccddeeff00112233445566778899aabbccdd",
    "file_version":{"type":"file_version","id":"98765",
        "sha1":"aabbccddeeff00112233445566778899aabbccdd"},
    "item_status":"active"}"#;

/// The same file after somebody uploaded a new version of it.
const FILE_NEW_VERSION: &str = r#"{"type":"file","id":"123456789","etag":"4","name":"release.bin",
    "size":1048576,"sha1":"00112233445566778899aabbccddeeff00112233",
    "file_version":{"type":"file_version","id":"98766",
        "sha1":"00112233445566778899aabbccddeeff00112233"},
    "item_status":"active"}"#;

/// One recorded request: where it went and what rode with it.
#[derive(Clone, Debug)]
struct Sent {
    path: String,
    query: Vec<(String, String)>,
    box_api: Option<String>,
}

/// The mock Box API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:box_access_token}}` and never as a value.
struct MockBox {
    status: u16,
    /// Answered in order; the last one is repeated once they run out.
    bodies: Vec<String>,
    answered: Mutex<usize>,
    requests: Mutex<Vec<Sent>>,
    authorizations: Mutex<Vec<String>>,
    has_token: bool,
}

impl MockBox {
    fn answering(status: u16, body: &str) -> Arc<Self> {
        Self::answering_in_turn(status, vec![body.to_owned()])
    }

    fn answering_in_turn(status: u16, bodies: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            status,
            bodies,
            answered: Mutex::new(0),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        })
    }

    fn signed_out() -> Arc<Self> {
        Arc::new(Self {
            status: 200,
            bodies: vec!["{}".to_owned()],
            answered: Mutex::new(0),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: false,
        })
    }

    fn requests(&self) -> Vec<Sent> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ResolverHost for MockBox {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let mut box_api = None;
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
            if header.name.eq_ignore_ascii_case("boxapi") {
                box_api = Some(header.value_template.clone());
            }
        }
        self.requests.lock().expect("requests").push(Sent {
            path: request.url.path().to_owned(),
            query: request
                .query
                .iter()
                .map(|value| (value.name.clone(), value.value_template.clone()))
                .collect(),
            box_api,
        });
        let account = request.url.path().ends_with("/users/me");
        let mut answered = self.answered.lock().expect("answered");
        let index = (*answered).min(self.bodies.len().saturating_sub(1));
        *answered += 1;
        Ok(HostHttpResponse {
            status: if account { 200 } else { self.status },
            final_url: request.url.clone(),
            headers: vec![rd_plugin_api::ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
            body: if account {
                br#"{"type":"user","id":"7","name":"Someone","login":"someone@example.invalid"}"#
                    .to_vec()
            } else {
                self.bodies[index].clone().into_bytes()
            },
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_token && reference == "box_access_token"
    }
}

fn resolver(host: Arc<MockBox>) -> BoxResolver {
    BoxResolver::new(host)
}

fn client(account: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account),
        proxy_profile_id: None,
        tls_revision: 7,
    }
}

async fn resolve(
    host: Arc<MockBox>,
    url: &str,
) -> Result<rd_plugin_api::ResolvedDownload, Failure> {
    let account = AccountId::new();
    resolver(host)
        .resolve(ResolveRequest {
            url: url.parse().expect("URL"),
            client: client(account),
        })
        .await
}

fn box_api(resolved: &rd_plugin_api::ResolvedDownload) -> Option<&str> {
    resolved
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("boxapi"))
        .map(|header| header.value.as_str())
}

/// A file in the account's own Box becomes the stable content address, pinned to the version
/// Box just described, with the name, size and SHA-1 it stated.
#[tokio::test]
async fn an_own_file_resolves_to_the_stable_download_address_pinned_to_its_version() {
    let host = MockBox::answering(200, FILE);
    let resolved = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://api.box.com/2.0/files/123456789/content?version=98765"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(1_048_576));
    let checksum = resolved.checksum.clone().expect("a SHA-1");
    assert_eq!(checksum.algorithm, rd_core::ChecksumAlgorithm::Sha1);
    assert_eq!(checksum.value, "aabbccddeeff00112233445566778899aabbccdd");
    assert_eq!(resolved.client.tls_revision, 7);
    // A file in the account's own Box carries no `boxapi` header: the bearer the scheduler
    // attaches is the whole of its authorization.
    assert_eq!(box_api(&resolved), None);
    // One metadata call, at the item the address named, asking for the fields and no more.
    let sent = host.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].path, "/2.0/files/123456789");
    assert_eq!(
        sent[0].query,
        vec![(
            "fields".to_owned(),
            "type,name,size,sha1,file_version,item_status".to_owned()
        )]
    );
}

/// Two versions of one file are two addresses. This is what keeps a resume from splicing the
/// head of one file onto the tail of another: the partial file came from the address the
/// version it was resolved at names, and a Box that has moved on answers a different one.
#[tokio::test]
async fn a_new_file_version_resolves_to_a_different_address_than_the_partial_file_came_from() {
    let host = MockBox::answering_in_turn(200, vec![FILE.to_owned(), FILE_NEW_VERSION.to_owned()]);
    let first = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect("resolved");
    let second = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect("resolved");

    assert_eq!(
        first.url.as_str(),
        "https://api.box.com/2.0/files/123456789/content?version=98765"
    );
    assert_eq!(
        second.url.as_str(),
        "https://api.box.com/2.0/files/123456789/content?version=98766"
    );
    assert_ne!(first.url, second.url);
    // And the digest the transfer is checked against moved with it, so a resume that did run to
    // the end against the wrong bytes still cannot be reported as complete.
    assert_ne!(
        first.checksum.expect("a SHA-1").value,
        second.checksum.expect("a SHA-1").value
    );
}

/// A shared link resolves through the same item route with the link in the `boxapi` header, and
/// a password-protected one carries its password there — and nowhere else.
#[tokio::test]
async fn a_shared_link_resolves_with_its_password_in_the_box_api_header_and_nowhere_else() {
    let host = MockBox::answering(200, FILE);
    let resolved = resolve(Arc::clone(&host), SHARED_FILE_WITH_PASSWORD)
        .await
        .expect("resolved");

    let expected = "shared_link=https://app.box.com/s/abc123def456&shared_link_password=hunter2";
    assert_eq!(box_api(&resolved), Some(expected));
    let sent = host.requests();
    assert_eq!(sent[0].path, "/2.0/files/42");
    assert_eq!(sent[0].box_api.as_deref(), Some(expected));
    // Not in the address the transfer goes to, not in a query parameter, not in the file name.
    assert!(!resolved.url.as_str().contains("hunter2"));
    assert!(!resolved.url.as_str().contains("shared_link"));
    assert!(
        !sent[0]
            .query
            .iter()
            .any(|(_, value)| value.contains("hunter2")),
        "{:?}",
        sent[0].query
    );

    // A shared link without a password carries only the link.
    let plain = resolve(MockBox::answering(200, FILE), SHARED_FILE)
        .await
        .expect("resolved");
    assert_eq!(
        box_api(&plain),
        Some("shared_link=https://app.box.com/s/abc123def456")
    );
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_marker_and_never_as_a_value() {
    let host = MockBox::answering(200, FILE);
    resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect("resolved");
    assert_eq!(
        host.authorizations.lock().expect("authorizations").clone(),
        vec!["Bearer {{secret:box_access_token}}".to_owned()]
    );
}

/// Each of Box's refusals arrives as its own code, because a person acts differently on each —
/// and nothing Box wrote travels into the message.
#[tokio::test]
async fn every_way_box_says_no_reaches_the_person_as_its_own_code() {
    let cases = [
        (
            404,
            r#"{"type":"error","status":404,"code":"not_found","message":"a sentence box wrote",
                "request_id":"abcdef123456"}"#,
            "box.file_not_found",
        ),
        (
            403,
            r#"{"type":"error","status":403,"code":"forbidden","message":"a sentence box wrote"}"#,
            "box.download_not_permitted",
        ),
        (
            403,
            r#"{"type":"error","status":403,"code":"storage_limit_exceeded"}"#,
            "box.quota_exceeded",
        ),
        (
            401,
            r#"{"type":"error","status":401,"code":"unauthorized"}"#,
            "box.sign_in_required",
        ),
        (
            429,
            r#"{"type":"error","status":429,"code":"rate_limit_exceeded"}"#,
            "box.rate_limited",
        ),
        (
            503,
            r#"{"type":"error","status":503,"code":"unavailable"}"#,
            "box.unavailable",
        ),
    ];
    for (status, body, expected) in cases {
        let failure = resolve(MockBox::answering(status, body), OWN_FILE)
            .await
            .expect_err("a refusal");
        assert_eq!(failure.code.as_deref(), Some(expected), "{status}");
        assert!(
            !failure.message.contains("a sentence box wrote"),
            "{}",
            failure.message
        );
        assert!(
            !failure.message.contains("abcdef123456"),
            "{}",
            failure.message
        );
    }
}

/// Box answers a wrong shared-link password with the same refusal it uses for a file somebody
/// may not read. Through a shared link it is said as a shared-link refusal, which is the one a
/// person can act on — and the password is not in it.
#[tokio::test]
async fn a_refusal_through_a_shared_link_names_the_link_and_not_the_password() {
    let failure = resolve(
        MockBox::answering(
            403,
            r#"{"type":"error","status":403,"code":"forbidden","message":"Forbidden"}"#,
        ),
        SHARED_FILE_WITH_PASSWORD,
    )
    .await
    .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("box.link_access_denied"));
    assert!(!failure.message.contains("hunter2"), "{}", failure.message);
    assert!(
        !failure
            .params
            .iter()
            .any(|(_, value)| value.contains("hunter2")),
        "{:?}",
        failure.params
    );
}

/// A rate limit holds the whole provider back — every Box link, and nothing else — for as long
/// as Box's own `Retry-After` says, not a second the scheduler decided for itself.
#[tokio::test]
async fn a_rate_limit_blocks_only_this_provider_for_the_time_box_asked() {
    let failure = resolve(
        MockBox::answering(
            429,
            r#"{"type":"error","status":429,"code":"rate_limit_exceeded"}"#,
        ),
        OWN_FILE,
    )
    .await
    .expect_err("a rate limit");
    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(90)
        }
    );
}

/// A folder pasted at the resolver says it is a folder, rather than "this file has no bytes".
#[tokio::test]
async fn a_folder_reached_through_a_file_address_says_it_is_one() {
    let failure = resolve(
        MockBox::answering(
            200,
            r#"{"type":"folder","id":"123456789","name":"Season 1","item_status":"active"}"#,
        ),
        OWN_FILE,
    )
    .await
    .expect_err("a folder is not a download");
    assert_eq!(failure.code.as_deref(), Some("box.is_a_folder"));
}

/// A file Box has put in the trash is gone, whatever else its document says.
#[tokio::test]
async fn a_trashed_file_is_not_offered_as_a_download() {
    let host = MockBox::answering(
        200,
        r#"{"type":"file","id":"123456789","name":"release.bin","size":1,
            "item_status":"trashed"}"#,
    );
    let failure = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect_err("trashed");
    assert_eq!(failure.code.as_deref(), Some("box.file_not_found"));
    assert_eq!(host.requests().len(), 1, "one metadata call and no more");
}

/// An account with no token asks Box nothing at all.
#[tokio::test]
async fn an_account_without_a_token_reaches_nothing() {
    let host = MockBox::signed_out();
    let failure = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect_err("not signed in");
    assert_eq!(failure.code.as_deref(), Some("box.sign_in_required"));
    assert!(
        host.requests().is_empty(),
        "a signed-out account must ask nothing"
    );
}

/// A link check reports what will arrive before anything is queued, and an address this plugin
/// does not claim is not reported as missing.
#[tokio::test]
async fn a_link_check_reports_the_name_and_size_the_file_will_arrive_with() {
    let account = AccountId::new();
    let checked = resolver(MockBox::answering(200, FILE))
        .check(CheckRequest {
            urls: vec![
                OWN_FILE.parse().expect("URL"),
                "https://ddownload.com/f/abc".parse().expect("URL"),
            ],
            client: client(account),
        })
        .await
        .expect("checked");
    assert_eq!(checked.len(), 2);
    assert_eq!(checked[0].status, rd_core::LinkStatus::Online);
    assert_eq!(checked[0].file_name.as_deref(), Some("release.bin"));
    assert_eq!(checked[0].size.map(|size| size.get()), Some(1_048_576));
    assert_eq!(checked[1].status, rd_core::LinkStatus::Unknown);
}

/// A file Box says is gone is offline; an outage says nothing and stays unknown.
#[tokio::test]
async fn a_missing_file_is_offline_and_an_outage_is_not() {
    let account = AccountId::new();
    let gone = resolver(MockBox::answering(
        404,
        r#"{"type":"error","status":404,"code":"not_found"}"#,
    ))
    .check(CheckRequest {
        urls: vec![OWN_FILE.parse().expect("URL")],
        client: client(account),
    })
    .await
    .expect("checked");
    assert_eq!(gone[0].status, rd_core::LinkStatus::Offline);

    let away = resolver(MockBox::answering(503, "{}"))
        .check(CheckRequest {
            urls: vec![OWN_FILE.parse().expect("URL")],
            client: client(account),
        })
        .await
        .expect("checked");
    assert_eq!(away[0].status, rd_core::LinkStatus::Unknown);
}

/// The account row shows the address somebody signed in with, and no quota number that would be
/// read as remaining download traffic.
#[tokio::test]
async fn the_account_reports_the_address_it_was_signed_in_with() {
    let status = resolver(MockBox::answering(200, "{}"))
        .check_account(AccountId::new())
        .await
        .expect("account");
    assert!(status.valid);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=someone@example.invalid)"
    );
    assert_eq!(status.traffic_left, None);
}

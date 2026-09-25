//! Native-adapter coverage for Dropbox, driven against a mock Dropbox API.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! dropboxapi.com is not contacted. A run against a real Dropbox account is *not* claimed
//! here; there is none in this checkout. What is proven is everything that does not need one:
//! which address turns into which request, what each of Dropbox's refusals becomes, that a
//! shared link's password reaches the official argument and nowhere else, and that the
//! account's token leaves this plugin as a marker rather than as a value.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};

use super::DropboxResolver;

const FILE_LINK: &str = "https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1&dl=0";
const OWN_FILE: &str = "https://www.dropbox.com/home/Show?preview=release.bin";

const FILE: &str = r#"{".tag":"file","name":"release.bin","id":"id:a1b2C3d4E5f6G7h8I9j0K",
    "path_display":"/Show/release.bin","rev":"015f3d2a1b2c3d4e5f6a7","size":1048576,
    "is_downloadable":true,
    "content_hash":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}"#;

/// One recorded request: the endpoint and the JSON body the plugin sent.
#[derive(Clone, Debug)]
struct Sent {
    endpoint: String,
    body: serde_json::Value,
}

/// The mock Dropbox API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:dropbox_access_token}}` and never as a value.
struct MockDropbox {
    status: u16,
    body: String,
    requests: Mutex<Vec<Sent>>,
    authorizations: Mutex<Vec<String>>,
    has_token: bool,
}

impl MockDropbox {
    fn answering(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            status,
            body: body.to_owned(),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        })
    }

    fn signed_out() -> Arc<Self> {
        Arc::new(Self {
            status: 200,
            body: "{}".to_owned(),
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
impl ResolverHost for MockDropbox {
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
        let body = serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
        self.requests.lock().expect("requests").push(Sent {
            endpoint: request.url.path().to_owned(),
            body,
        });
        let account = request.url.path().ends_with("/users/get_current_account");
        Ok(HostHttpResponse {
            status: if account { 200 } else { self.status },
            final_url: request.url.clone(),
            headers: vec![rd_plugin_api::ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
            body: if account {
                br#"{"email":"someone@example.invalid","name":{"display_name":"Someone"}}"#.to_vec()
            } else {
                self.body.clone().into_bytes()
            },
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_token && reference == "dropbox_access_token"
    }
}

fn resolver(host: Arc<MockDropbox>) -> DropboxResolver {
    DropboxResolver::new(host)
}

fn client(account: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account),
        proxy_profile_id: None,
        tls_revision: 7,
    }
}

async fn resolve(
    host: Arc<MockDropbox>,
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

fn api_arg(resolved: &rd_plugin_api::ResolvedDownload) -> serde_json::Value {
    let header = resolved
        .headers
        .iter()
        .find(|header| header.name == "Dropbox-API-Arg")
        .expect("the API argument header");
    assert!(
        header.value.is_ascii(),
        "a header must be ASCII: {}",
        header.value
    );
    serde_json::from_str(&header.value).expect("the argument is JSON")
}

/// A file in the account's own Dropbox becomes the stable content address, with the revision
/// Dropbox just described named in the header, and the name, size and content hash it stated.
#[tokio::test]
async fn an_own_file_resolves_to_the_stable_download_address_pinned_to_its_revision() {
    let host = MockDropbox::answering(200, FILE);
    let resolved = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://content.dropboxapi.com/2/files/download"
    );
    assert_eq!(
        api_arg(&resolved),
        serde_json::json!({"path": "rev:015f3d2a1b2c3d4e5f6a7"})
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(1_048_576));
    let checksum = resolved.checksum.expect("a content hash");
    assert_eq!(
        checksum.algorithm,
        rd_core::ChecksumAlgorithm::DropboxContentHash
    );
    assert_eq!(
        checksum.value,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(resolved.client.tls_revision, 7);
    // One metadata call, at the path the address named.
    let sent = host.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].endpoint, "/2/files/get_metadata");
    assert_eq!(
        sent[0].body,
        serde_json::json!({"path": "/Show/release.bin"})
    );
}

/// A shared link resolves through the sharing endpoints, and a password-protected one carries
/// its password in the official argument — and nowhere else.
#[tokio::test]
async fn a_shared_link_resolves_through_the_sharing_endpoint_with_its_password() {
    let host = MockDropbox::answering(200, FILE);
    let resolved = resolve(
        Arc::clone(&host),
        "https://www.dropbox.com/scl/fo/abc/h1/Season%201?rlkey=k1&preview=e01.mkv&link_password=hunter2",
    )
    .await
    .expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        "https://content.dropboxapi.com/2/sharing/get_shared_link_file"
    );
    let expected = serde_json::json!({
        "url": "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1",
        "path": "/Season 1/e01.mkv",
        "link_password": "hunter2"
    });
    assert_eq!(api_arg(&resolved), expected);
    let sent = host.requests();
    assert_eq!(sent[0].endpoint, "/2/sharing/get_shared_link_metadata");
    assert_eq!(sent[0].body, expected);
    // The password is not in the address the transfer goes to.
    assert!(!resolved.url.as_str().contains("hunter2"));

    // A plain file link needs no path and no password.
    let plain = resolve(MockDropbox::answering(200, FILE), FILE_LINK)
        .await
        .expect("resolved");
    assert_eq!(
        api_arg(&plain),
        serde_json::json!({"url": "https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1"})
    );
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_marker_and_never_as_a_value() {
    let host = MockDropbox::answering(200, FILE);
    resolve(Arc::clone(&host), FILE_LINK)
        .await
        .expect("resolved");
    assert_eq!(
        host.authorizations.lock().expect("authorizations").clone(),
        vec!["Bearer {{secret:dropbox_access_token}}".to_owned()]
    );
}

/// Each of Dropbox's refusals arrives as its own code, because a person acts differently on
/// each — and nothing Dropbox wrote travels into the message.
#[tokio::test]
async fn every_way_dropbox_says_no_reaches_the_person_as_its_own_code() {
    let cases = [
        (
            409,
            r#"{"error_summary":"path/not_found/..","error":{".tag":"path","path":{".tag":"not_found"}},"user_message":"a sentence Dropbox wrote"}"#,
            "dropbox.file_not_found",
        ),
        (
            409,
            r#"{"error_summary":"shared_link_access_denied/..","error":{".tag":"shared_link_access_denied"}}"#,
            "dropbox.link_access_denied",
        ),
        (
            409,
            r#"{"error_summary":"path/restricted_content/..","error":{".tag":"path","path":{".tag":"restricted_content"}}}"#,
            "dropbox.download_not_permitted",
        ),
        (
            401,
            r#"{"error_summary":"expired_access_token/","error":{".tag":"expired_access_token"}}"#,
            "dropbox.sign_in_required",
        ),
        (
            429,
            r#"{"error_summary":"too_many_requests/..","error":{"reason":{".tag":"too_many_requests"},"retry_after":300}}"#,
            "dropbox.rate_limited",
        ),
    ];
    for (status, body, expected) in cases {
        let failure = resolve(MockDropbox::answering(status, body), FILE_LINK)
            .await
            .expect_err("a refusal");
        assert_eq!(failure.code.as_deref(), Some(expected), "{status}");
        assert!(
            !failure.message.contains("a sentence Dropbox wrote"),
            "{}",
            failure.message
        );
    }
}

/// A rate limit holds the whole provider back — every Dropbox link, and nothing else — for as
/// long as Dropbox's own `Retry-After` says, not a second the scheduler decided for itself.
#[tokio::test]
async fn a_rate_limit_blocks_only_this_provider_for_the_time_dropbox_asked() {
    let failure = resolve(
        MockDropbox::answering(
            429,
            r#"{"error":{"reason":{".tag":"too_many_requests"},"retry_after":300}}"#,
        ),
        FILE_LINK,
    )
    .await
    .expect_err("a rate limit");
    // The header wins over the document; the mock sends 90 in the header.
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
        MockDropbox::answering(200, r#"{".tag":"folder","name":"Season 1","id":"id:f1"}"#),
        OWN_FILE,
    )
    .await
    .expect_err("a folder is not a download");
    assert_eq!(failure.code.as_deref(), Some("dropbox.is_a_folder"));
}

/// A file Dropbox will not serve bytes for is refused before a download is attempted.
#[tokio::test]
async fn a_file_without_bytes_is_refused_before_a_download_is_attempted() {
    let host = MockDropbox::answering(
        200,
        r#"{".tag":"file","name":"Notes","is_downloadable":false}"#,
    );
    let failure = resolve(Arc::clone(&host), OWN_FILE)
        .await
        .expect_err("not downloadable");
    assert_eq!(
        failure.code.as_deref(),
        Some("dropbox.download_not_permitted")
    );
    assert_eq!(host.requests().len(), 1, "one metadata call and no more");
}

/// An account with no token asks Dropbox nothing at all.
#[tokio::test]
async fn an_account_without_a_token_reaches_nothing() {
    let host = MockDropbox::signed_out();
    let failure = resolve(Arc::clone(&host), FILE_LINK)
        .await
        .expect_err("not signed in");
    assert_eq!(failure.code.as_deref(), Some("dropbox.sign_in_required"));
    assert!(
        host.requests().is_empty(),
        "a signed-out account must ask nothing"
    );
}

/// A link check reports what will arrive before anything is queued, and an address this
/// plugin does not claim is not reported as missing.
#[tokio::test]
async fn a_link_check_reports_the_name_and_size_the_file_will_arrive_with() {
    let account = AccountId::new();
    let checked = resolver(MockDropbox::answering(200, FILE))
        .check(CheckRequest {
            urls: vec![
                FILE_LINK.parse().expect("URL"),
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

/// A file Dropbox says is gone is offline; anything else says nothing and stays unknown.
#[tokio::test]
async fn a_missing_file_is_offline_and_an_outage_is_not() {
    let account = AccountId::new();
    let gone = resolver(MockDropbox::answering(
        409,
        r#"{"error":{".tag":"shared_link_not_found"}}"#,
    ))
    .check(CheckRequest {
        urls: vec![FILE_LINK.parse().expect("URL")],
        client: client(account),
    })
    .await
    .expect("checked");
    assert_eq!(gone[0].status, rd_core::LinkStatus::Offline);

    let away = resolver(MockDropbox::answering(503, "{}"))
        .check(CheckRequest {
            urls: vec![FILE_LINK.parse().expect("URL")],
            client: client(account),
        })
        .await
        .expect("checked");
    assert_eq!(away[0].status, rd_core::LinkStatus::Unknown);
}

/// The account row shows the address somebody signed in with, and no quota number that would
/// be read as remaining download traffic.
#[tokio::test]
async fn the_account_reports_the_address_it_was_signed_in_with() {
    let status = resolver(MockDropbox::answering(200, "{}"))
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

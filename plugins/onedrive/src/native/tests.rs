//! Native-adapter coverage for OneDrive, driven against a mock Graph API.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! graph.microsoft.com is not contacted. A run against a real Microsoft account is *not*
//! claimed here; there is none in this checkout. What is proven is everything that does not
//! need one: which address turns into which Graph route, what each of Graph's refusals
//! becomes, that the download address is the stable `/content` route and never the
//! pre-authenticated one, and that the account's token leaves this plugin as a marker rather
//! than as a value.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};

use super::OneDriveResolver;

const SHARE_LINK: &str = "https://1drv.ms/u/s!AkXy_Zabc-DEF";
const SHARE: &str = "u!aHR0cHM6Ly8xZHJ2Lm1zL3UvcyFBa1h5X1phYmMtREVG";
const ITEM_ID: &str = "01BYE5RZ6QN3ZWBTUFOFD3GSPGOHDJD36K";

/// An item as the mock holds it: the answer, and the status it comes back with.
struct Answer {
    status: u16,
    body: String,
}

/// The mock Graph API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:onedrive_access_token}}` and never as a value.
struct MockGraph {
    item: Answer,
    drive: Answer,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    has_token: bool,
}

impl MockGraph {
    fn with_item(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            item: Answer {
                status,
                body: body.to_owned(),
            },
            drive: Answer {
                status: 200,
                body: r#"{"id":"b!abc","driveType":"personal",
                          "owner":{"user":{"displayName":"Someone","email":"someone@example.invalid"}}}"#
                    .to_owned(),
            },
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        })
    }

    fn signed_out() -> Arc<Self> {
        let mut mock = Self {
            item: Answer {
                status: 200,
                body: "{}".to_owned(),
            },
            drive: Answer {
                status: 200,
                body: "{}".to_owned(),
            },
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        };
        mock.has_token = false;
        Arc::new(mock)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ResolverHost for MockGraph {
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
        let select = request
            .query
            .iter()
            .find(|value| value.name == "$select")
            .map(|value| value.value_template.clone())
            .unwrap_or_default();
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {}", request.url.path(), select));
        let answer = if request.url.path().ends_with("/me/drive") {
            &self.drive
        } else {
            &self.item
        };
        Ok(HostHttpResponse {
            status: answer.status,
            final_url: request.url.clone(),
            headers: vec![rd_plugin_api::ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: "90".to_owned(),
            }],
            body: answer.body.clone().into_bytes(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_token && reference == "onedrive_access_token"
    }
}

fn resolver(host: Arc<MockGraph>) -> OneDriveResolver {
    OneDriveResolver::new(host)
}

fn client(account: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account),
        proxy_profile_id: None,
        tls_revision: 7,
    }
}

async fn resolve(
    host: Arc<MockGraph>,
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

fn file_body() -> String {
    format!(
        r#"{{"id":"{ITEM_ID}","name":"release.bin","size":1048576,
             "eTag":"\"{{6FBF2E7F}},2\"","cTag":"\"c:{{6FBF2E7F}},1\"",
             "file":{{"mimeType":"application/octet-stream",
                      "hashes":{{"quickXorHash":"MjJhOTk4ZjM0NWQ2","sha1Hash":"DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"}}}},
             "@microsoft.graph.downloadUrl":"https://example.invalid/short-lived?tempauth=SECRET"}}"#
    )
}

/// A sharing link becomes the stable `/content` route of the item behind it, with the name,
/// the size and the checksum Graph stated — and never the pre-authenticated address.
#[tokio::test]
async fn a_sharing_link_resolves_to_the_stable_content_route() {
    let host = MockGraph::with_item(200, &file_body());
    let resolved = resolve(Arc::clone(&host), SHARE_LINK)
        .await
        .expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        format!("https://graph.microsoft.com/v1.0/shares/{SHARE}/driveItem/content")
    );
    assert!(
        !resolved.url.as_str().contains("tempauth"),
        "the pre-authenticated address must never be the download address: {}",
        resolved.url
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(1_048_576));
    assert_eq!(
        resolved.checksum.map(|checksum| checksum.value),
        Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned())
    );
    assert_eq!(resolved.client.tls_revision, 7);
    // The sharing link was handed to Graph whole, as the share id Microsoft documents, and
    // the metadata call named the facets it needs.
    let requests = host.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with(&format!("/v1.0/shares/{SHARE}/driveItem ")),
        "{requests:?}"
    );
    assert!(requests[0].contains("folder"), "{requests:?}");
}

/// The canonical per-item address the sibling crawler emits resolves through the share it was
/// found in, so a link shared with the account keeps granting access to what is inside it.
#[tokio::test]
async fn an_item_inside_a_shared_folder_is_read_through_its_share() {
    let host = MockGraph::with_item(200, &file_body());
    let resolved = resolve(
        Arc::clone(&host),
        &onedrive_common::address::item_address(SHARE, ITEM_ID),
    )
    .await
    .expect("resolved");
    assert_eq!(
        resolved.url.as_str(),
        format!("https://graph.microsoft.com/v1.0/shares/{SHARE}/items/{ITEM_ID}/content")
    );
    // And an item by its own drive and id goes straight to it.
    let direct = resolve(
        MockGraph::with_item(200, &file_body()),
        &format!("https://graph.microsoft.com/v1.0/drives/b!abc/items/{ITEM_ID}"),
    )
    .await
    .expect("resolved");
    assert_eq!(
        direct.url.as_str(),
        format!("https://graph.microsoft.com/v1.0/drives/b!abc/items/{ITEM_ID}/content")
    );
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_marker_and_never_as_a_value() {
    let host = MockGraph::with_item(200, &file_body());
    resolve(Arc::clone(&host), SHARE_LINK)
        .await
        .expect("resolved");
    assert_eq!(
        host.authorizations.lock().expect("authorizations").clone(),
        vec!["Bearer {{secret:onedrive_access_token}}".to_owned()]
    );
}

/// Each of Graph's refusals arrives as its own code, because a person acts differently on
/// each — and two of them arrive as the same HTTP 403.
#[tokio::test]
async fn every_way_graph_says_no_reaches_the_person_as_its_own_code() {
    let cases = [
        (403, "accessDenied", "onedrive.access_denied"),
        (403, "notAllowed", "onedrive.download_not_permitted"),
        (403, "malwareDetected", "onedrive.malware_detected"),
        (404, "itemNotFound", "onedrive.item_not_found"),
        (400, "invalidRequest", "onedrive.invalid_request"),
        (429, "activityLimitReached", "onedrive.rate_limited"),
        (
            401,
            "InvalidAuthenticationToken",
            "onedrive.sign_in_required",
        ),
    ];
    for (status, code, expected) in cases {
        let body = format!(
            r#"{{"error":{{"code":"{code}","message":"a sentence Microsoft wrote",
                 "innerError":{{"request-id":"r","date":"d"}}}}}}"#
        );
        let failure = resolve(MockGraph::with_item(status, &body), SHARE_LINK)
            .await
            .expect_err("a refusal");
        assert_eq!(failure.code.as_deref(), Some(expected), "{code}");
        // Nothing Microsoft wrote travels into the message.
        assert!(
            !failure.message.contains("a sentence Microsoft wrote"),
            "{}",
            failure.message
        );
    }
}

/// A throttle carries Microsoft's own `Retry-After` so the scheduler waits as long as it was
/// asked to, and does not decide for itself.
#[tokio::test]
async fn a_throttle_carries_the_retry_after_graph_sent() {
    let failure = resolve(
        MockGraph::with_item(
            429,
            r#"{"error":{"code":"activityLimitReached","message":"throttled"}}"#,
        ),
        SHARE_LINK,
    )
    .await
    .expect_err("a throttle");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(90)
        }
    );
}

/// A folder pasted at the resolver says it is a folder, rather than "this item has no bytes",
/// and a OneNote notebook says it is not a file at all.
#[tokio::test]
async fn a_folder_and_a_notebook_say_what_they_are() {
    let folder = resolve(
        MockGraph::with_item(
            200,
            r#"{"id":"x","name":"Season 1","folder":{"childCount":12}}"#,
        ),
        SHARE_LINK,
    )
    .await
    .expect_err("a folder is not a download");
    assert_eq!(folder.code.as_deref(), Some("onedrive.is_a_folder"));

    let notebook = resolve(
        MockGraph::with_item(
            200,
            r#"{"id":"x","name":"Notes","package":{"type":"oneNote"}}"#,
        ),
        SHARE_LINK,
    )
    .await
    .expect_err("a notebook is not a download");
    assert_eq!(notebook.code.as_deref(), Some("onedrive.not_a_file"));
}

/// A recycled item still answers with a tombstone, and a tombstone is not a file.
#[tokio::test]
async fn a_deleted_item_is_not_there() {
    let failure = resolve(
        MockGraph::with_item(
            200,
            r#"{"id":"x","name":"old.bin","file":{},"deleted":{"state":"deleted"}}"#,
        ),
        SHARE_LINK,
    )
    .await
    .expect_err("deleted");
    assert_eq!(failure.code.as_deref(), Some("onedrive.item_not_found"));
}

/// An account with no token asks Graph nothing at all.
#[tokio::test]
async fn an_account_without_a_token_reaches_nothing() {
    let host = MockGraph::signed_out();
    let failure = resolve(Arc::clone(&host), SHARE_LINK)
        .await
        .expect_err("not signed in");
    assert_eq!(failure.code.as_deref(), Some("onedrive.sign_in_required"));
    assert!(
        host.requests().is_empty(),
        "a signed-out account must ask nothing"
    );
}

/// A link check reports what will arrive before anything is queued, and leaves a stranger's
/// address alone.
#[tokio::test]
async fn a_link_check_reports_the_name_and_size_the_file_will_arrive_with() {
    let host = MockGraph::with_item(200, &file_body());
    let account = AccountId::new();
    let checked = resolver(host)
        .check(CheckRequest {
            urls: vec![
                SHARE_LINK.parse().expect("URL"),
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
    // An address this plugin does not claim is not reported as missing.
    assert_eq!(checked[1].status, rd_core::LinkStatus::Unknown);
}

/// An item Graph says is gone is offline; anything else says nothing and stays unknown.
#[tokio::test]
async fn a_missing_item_is_offline_and_an_outage_is_not() {
    let account = AccountId::new();
    let gone = resolver(MockGraph::with_item(
        404,
        r#"{"error":{"code":"itemNotFound","message":"gone"}}"#,
    ))
    .check(CheckRequest {
        urls: vec![SHARE_LINK.parse().expect("URL")],
        client: client(account),
    })
    .await
    .expect("checked");
    assert_eq!(gone[0].status, rd_core::LinkStatus::Offline);

    let away = resolver(MockGraph::with_item(503, "{}"))
        .check(CheckRequest {
            urls: vec![SHARE_LINK.parse().expect("URL")],
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
    let host = MockGraph::with_item(200, "{}");
    let status = resolver(Arc::clone(&host))
        .check_account(AccountId::new())
        .await
        .expect("account");
    assert!(status.valid);
    assert_eq!(
        plugin_common::native::label_summary(&status.label),
        "plugin.account.user(user=someone@example.invalid)"
    );
    assert_eq!(status.traffic_left, None);
    assert!(host.requests()[0].starts_with("/v1.0/me/drive "));
}

//! Native-adapter coverage for Google Drive, driven against a mock Drive API.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! googleapis.com is not contacted. A run against a real Google account is *not* claimed here;
//! there is none in this checkout. What is proven is everything that does not need one: which
//! address turns into which request, what each of Drive's refusals becomes, that a Workspace
//! document's name and extension are decided before anything is queued, and that the account's
//! token leaves this plugin as a marker rather than as a value.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};

use super::GoogleDriveResolver;

const FILE_ID: &str = "1A2b3C4d5E6f7G8h9I0j";

/// A file as the mock holds it: the `files.get` answer, and the status it comes back with.
struct Answer {
    status: u16,
    body: String,
}

/// The mock Drive API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:google_drive_access_token}}` and never as a value.
struct MockDrive {
    files: Answer,
    about: Answer,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    has_token: bool,
}

impl MockDrive {
    fn with_file(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            files: Answer {
                status,
                body: body.to_owned(),
            },
            about: Answer {
                status: 200,
                body: r#"{"user":{"emailAddress":"someone@example.invalid"}}"#.to_owned(),
            },
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        })
    }

    fn signed_out() -> Arc<Self> {
        let mut mock = Self {
            files: Answer {
                status: 200,
                body: "{}".to_owned(),
            },
            about: Answer {
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
impl ResolverHost for MockDrive {
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
        let fields = request
            .query
            .iter()
            .find(|value| value.name == "fields")
            .map(|value| value.value_template.clone())
            .unwrap_or_default();
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {}", request.url.path(), fields));
        let answer = if request.url.path().ends_with("/about") {
            &self.about
        } else {
            &self.files
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
        self.has_token && reference == "google_drive_access_token"
    }
}

fn resolver(host: Arc<MockDrive>) -> GoogleDriveResolver {
    GoogleDriveResolver::new(host)
}

fn client(account: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account),
        proxy_profile_id: None,
        tls_revision: 7,
    }
}

async fn resolve(
    host: Arc<MockDrive>,
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

/// A shared link becomes the API address its bytes come from, with the name, the size and the
/// checksum Drive stated — and the client identity it was bound to.
#[tokio::test]
async fn a_shared_link_resolves_to_the_api_media_address() {
    let host = MockDrive::with_file(
        200,
        &format!(
            r#"{{"id":"{FILE_ID}","name":"release.bin","mimeType":"application/octet-stream",
                 "size":"1048576","md5Checksum":"d41d8cd98f00b204e9800998ecf8427e",
                 "capabilities":{{"canDownload":true}}}}"#
        ),
    );
    let resolved = resolve(
        Arc::clone(&host),
        &format!("https://drive.google.com/file/d/{FILE_ID}/view?usp=sharing"),
    )
    .await
    .expect("resolved");

    assert_eq!(
        resolved.url.as_str(),
        format!(
            "https://www.googleapis.com/drive/v3/files/{FILE_ID}?alt=media&supportsAllDrives=true"
        )
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(1_048_576));
    assert_eq!(
        resolved.checksum.map(|checksum| checksum.value),
        Some("d41d8cd98f00b204e9800998ecf8427e".to_owned())
    );
    assert_eq!(resolved.client.tls_revision, 7);
    // `fields` is not an optimisation: without it Drive answers with a record that has no size.
    assert!(host.requests()[0].contains("size"), "{:?}", host.requests());
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_marker_and_never_as_a_value() {
    let host = MockDrive::with_file(
        200,
        r#"{"name":"a.bin","mimeType":"application/octet-stream"}"#,
    );
    resolve(
        Arc::clone(&host),
        &format!("https://drive.google.com/file/d/{FILE_ID}/view"),
    )
    .await
    .expect("resolved");
    assert_eq!(
        host.authorizations.lock().expect("authorizations").clone(),
        vec!["Bearer {{secret:google_drive_access_token}}".to_owned()]
    );
}

/// A Workspace document becomes an export, and its name carries the extension it will arrive
/// with — decided here, before anything is queued, because the LinkGrabber is the only place
/// somebody can notice that their spreadsheet is about to become a PDF.
#[tokio::test]
async fn a_workspace_document_resolves_to_an_export_with_its_extension_in_the_name() {
    let sheet = r#"{"name":"Quarterly numbers",
        "mimeType":"application/vnd.google-apps.spreadsheet","capabilities":{"canDownload":true}}"#;

    let resolved = resolve(
        MockDrive::with_file(200, sheet),
        &format!("https://docs.google.com/spreadsheets/d/{FILE_ID}/edit"),
    )
    .await
    .expect("resolved");
    assert_eq!(
        resolved.file_name.as_deref(),
        Some("Quarterly numbers.xlsx")
    );
    assert!(
        resolved.url.as_str().starts_with(&format!(
            "https://www.googleapis.com/drive/v3/files/{FILE_ID}/export?mimeType="
        )),
        "{}",
        resolved.url
    );
    // No invented size and no invented checksum: the bytes do not exist until the export runs.
    assert_eq!(resolved.size, None);
    assert!(resolved.checksum.is_none());

    // And the address that names a format gets that format instead.
    let as_pdf = resolve(
        MockDrive::with_file(200, sheet),
        &format!("https://docs.google.com/spreadsheets/d/{FILE_ID}/export?format=pdf"),
    )
    .await
    .expect("resolved");
    assert_eq!(as_pdf.file_name.as_deref(), Some("Quarterly numbers.pdf"));
    assert!(
        as_pdf.url.as_str().contains("application%2Fpdf"),
        "{}",
        as_pdf.url
    );
}

/// A document type Drive exports nothing for is a refusal with its own code, not an empty file.
#[tokio::test]
async fn a_document_type_with_no_export_is_refused_by_name() {
    let failure = resolve(
        MockDrive::with_file(
            200,
            r#"{"name":"Signup","mimeType":"application/vnd.google-apps.form"}"#,
        ),
        &format!("https://docs.google.com/forms/d/{FILE_ID}/edit"),
    )
    .await
    .expect_err("a Form cannot be downloaded");
    assert_eq!(
        failure.code.as_deref(),
        Some("google_drive.export_unsupported")
    );

    let wrong_format = resolve(
        MockDrive::with_file(
            200,
            r#"{"name":"Notes","mimeType":"application/vnd.google-apps.spreadsheet"}"#,
        ),
        &format!("https://docs.google.com/spreadsheets/d/{FILE_ID}/export?format=epub"),
    )
    .await
    .expect_err("a Sheet does not export to EPUB");
    assert_eq!(
        wrong_format.code.as_deref(),
        Some("google_drive.export_format_unsupported")
    );
}

/// Each of Drive's refusals arrives as its own code, because a person acts differently on each.
#[tokio::test]
async fn every_way_drive_says_no_reaches_the_person_as_its_own_code() {
    let cases = [
        ("downloadQuotaExceeded", "google_drive.quota_exceeded", 403),
        (
            "cannotDownloadAbusiveFile",
            "google_drive.virus_scan_warning",
            403,
        ),
        (
            "cannotDownloadFile",
            "google_drive.download_not_permitted",
            403,
        ),
        ("userRateLimitExceeded", "google_drive.rate_limited", 429),
        ("notFound", "google_drive.file_not_found", 404),
    ];
    for (reason, expected, status) in cases {
        let body = format!(
            r#"{{"error":{{"code":{status},"message":"a sentence Google wrote",
                 "errors":[{{"domain":"usageLimits","reason":"{reason}"}}]}}}}"#
        );
        let failure = resolve(
            MockDrive::with_file(status, &body),
            &format!("https://drive.google.com/file/d/{FILE_ID}/view"),
        )
        .await
        .expect_err("a refusal");
        assert_eq!(failure.code.as_deref(), Some(expected), "{reason}");
        // Nothing Google wrote travels into the message.
        assert!(
            !failure.message.contains("a sentence Google wrote"),
            "{}",
            failure.message
        );
    }
}

/// A rate limit carries Google's own `Retry-After` so the scheduler waits as long as it was
/// asked to, and does not decide for itself.
#[tokio::test]
async fn a_rate_limit_carries_the_retry_after_google_sent() {
    let failure = resolve(
        MockDrive::with_file(
            429,
            r#"{"error":{"code":429,"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
        ),
        &format!("https://drive.google.com/file/d/{FILE_ID}/view"),
    )
    .await
    .expect_err("a rate limit");
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: Some(90)
        }
    );
}

/// A folder pasted at the resolver says it is a folder, rather than "this file has no bytes".
#[tokio::test]
async fn a_folder_reached_through_an_api_address_says_it_is_one() {
    let failure = resolve(
        MockDrive::with_file(
            200,
            r#"{"name":"Season 1","mimeType":"application/vnd.google-apps.folder"}"#,
        ),
        &format!("https://www.googleapis.com/drive/v3/files/{FILE_ID}?alt=media"),
    )
    .await
    .expect_err("a folder is not a download");
    assert_eq!(failure.code.as_deref(), Some("google_drive.is_a_folder"));
}

/// An owner who switched downloading off is told apart from a file that is not there.
#[tokio::test]
async fn a_file_the_account_may_only_look_at_is_refused_before_a_download_is_attempted() {
    let host = MockDrive::with_file(
        200,
        r#"{"name":"internal.pdf","mimeType":"application/pdf",
             "capabilities":{"canDownload":false}}"#,
    );
    let failure = resolve(
        Arc::clone(&host),
        &format!("https://drive.google.com/file/d/{FILE_ID}/view"),
    )
    .await
    .expect_err("view-only");
    assert_eq!(
        failure.code.as_deref(),
        Some("google_drive.download_not_permitted")
    );
    assert_eq!(host.requests().len(), 1, "one metadata call and no more");
}

/// An account with no token asks Google nothing at all.
#[tokio::test]
async fn an_account_without_a_token_reaches_nothing() {
    let host = MockDrive::signed_out();
    let failure = resolve(
        Arc::clone(&host),
        &format!("https://drive.google.com/file/d/{FILE_ID}/view"),
    )
    .await
    .expect_err("not signed in");
    assert_eq!(
        failure.code.as_deref(),
        Some("google_drive.sign_in_required")
    );
    assert!(
        host.requests().is_empty(),
        "a signed-out account must ask nothing"
    );
}

/// A link check reports what will arrive, including the export name, before anything is queued.
#[tokio::test]
async fn a_link_check_reports_the_name_the_file_will_arrive_under() {
    let host = MockDrive::with_file(
        200,
        r#"{"name":"Board deck","mimeType":"application/vnd.google-apps.presentation"}"#,
    );
    let account = AccountId::new();
    let checked = resolver(host)
        .check(CheckRequest {
            urls: vec![
                format!("https://docs.google.com/presentation/d/{FILE_ID}/edit")
                    .parse()
                    .expect("URL"),
                "https://ddownload.com/f/abc".parse().expect("URL"),
            ],
            client: client(account),
        })
        .await
        .expect("checked");
    assert_eq!(checked.len(), 2);
    assert_eq!(checked[0].status, rd_core::LinkStatus::Online);
    assert_eq!(checked[0].file_name.as_deref(), Some("Board deck.pptx"));
    // An address this plugin does not claim is not reported as missing.
    assert_eq!(checked[1].status, rd_core::LinkStatus::Unknown);
}

/// A file Drive says is gone is offline; anything else says nothing and stays unknown.
#[tokio::test]
async fn a_missing_file_is_offline_and_an_outage_is_not() {
    let account = AccountId::new();
    let gone = resolver(MockDrive::with_file(
        404,
        r#"{"error":{"code":404,"errors":[{"reason":"notFound"}]}}"#,
    ))
    .check(CheckRequest {
        urls: vec![
            format!("https://drive.google.com/file/d/{FILE_ID}/view")
                .parse()
                .expect("URL"),
        ],
        client: client(account),
    })
    .await
    .expect("checked");
    assert_eq!(gone[0].status, rd_core::LinkStatus::Offline);

    let away = resolver(MockDrive::with_file(503, "{}"))
        .check(CheckRequest {
            urls: vec![
                format!("https://drive.google.com/file/d/{FILE_ID}/view")
                    .parse()
                    .expect("URL"),
            ],
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
    let status = resolver(MockDrive::with_file(200, "{}"))
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

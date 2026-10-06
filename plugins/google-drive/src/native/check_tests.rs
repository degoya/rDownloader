//! The link check and the account row, against the mock Drive API in `tests.rs`; split from it
//! to keep both files under the crate layout's 500 lines.

use rd_core::AccountId;
use rd_plugin_api::{CheckRequest, Resolver};

use super::{FILE_ID, MockDrive, client, resolver};

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

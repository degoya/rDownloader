//! The link check and the account row, against the mock Box Content API in `tests.rs`; split
//! from it to keep both files under the crate layout's 500 lines.

use rd_core::AccountId;
use rd_plugin_api::{CheckRequest, Resolver};

use super::{FILE, MockBox, OWN_FILE, client, resolver};

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

//! `check` on the scripted host: the batch, its mapping, and its refusals.

use rd_core::{FailureKind, LinkStatus};
use rd_plugin_api::{CheckRequest, ClientIdentity, Resolver};

use super::{API_ERROR_261, GET_INFO_BATCH, GET_INFO_INVALID, MockHost, json, resolver};

fn check_request(urls: &[&str]) -> CheckRequest {
    CheckRequest {
        urls: urls.iter().map(|url| url.parse().expect("URL")).collect(),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

fn keys_sent(host: &MockHost, index: usize) -> Vec<String> {
    host.requests()[index]
        .query
        .iter()
        .find(|item| item.name == "quick_key")
        .map(|item| item.value_template.split(',').map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The measured batch answer: one key found, one skipped; plus a link that is no key at all.
#[tokio::test]
async fn a_batch_maps_found_skipped_and_unparsable_links() {
    let host = MockHost::with_responses(vec![json(200, GET_INFO_BATCH)]);
    let results = resolver(&host)
        .check(check_request(&[
            "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file",
            "https://www.mediafire.com/?uz9u9zqa0tlk6z7",
            "https://www.mediafire.com/folder/rww7bhhi0yc1l",
        ]))
        .await
        .expect("checked");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("test-10mb.bin"));
    assert_eq!(
        results[0].size.map(rd_core::ByteCount::get),
        Some(10_485_760)
    );
    assert_eq!(results[1].status, LinkStatus::Offline);
    assert_eq!(results[1].file_name, None);
    assert_eq!(results[2].status, LinkStatus::Unknown);

    let requests = host.requests();
    assert_eq!(requests.len(), 1, "one call for the whole batch");
    assert_eq!(
        keys_sent(&host, 0),
        vec!["ipnyzofjcwri357".to_owned(), "uz9u9zqa0tlk6z7".to_owned()],
        "the folder link sends no key"
    );
}

/// The API's own limit is 500 keys, JD's 100; a check of more is split.
#[tokio::test]
async fn more_than_a_hundred_keys_are_split_into_calls_of_a_hundred() {
    let urls: Vec<String> = (0..150)
        .map(|index| format!("https://www.mediafire.com/file/{index:0>15}"))
        .collect();
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let empty = br#"{"response":{"action":"file/get_info","file_infos":[],"result":"Success"}}"#;
    let host = MockHost::with_responses(vec![json(200, empty), json(200, empty)]);
    let results = resolver(&host)
        .check(check_request(&refs))
        .await
        .expect("checked");
    assert_eq!(results.len(), 150);
    assert!(
        results
            .iter()
            .all(|result| result.status == LinkStatus::Offline)
    );
    assert_eq!(keys_sent(&host, 0).len(), 100);
    assert_eq!(keys_sent(&host, 1).len(), 50);
}

/// A single unknown key is refused as a whole with 110, which is "offline", not a failure.
#[tokio::test]
async fn a_whole_call_refused_with_110_is_offline() {
    let host = MockHost::with_responses(vec![json(404, GET_INFO_INVALID)]);
    let results = resolver(&host)
        .check(check_request(&[
            "https://www.mediafire.com/file/uz9u9zqa0tlk6z7",
        ]))
        .await
        .expect("checked");
    assert_eq!(results[0].status, LinkStatus::Offline);
}

/// A rate limit is a failure of the batch, with the category the scheduler backs off on.
#[tokio::test]
async fn a_rate_limited_batch_fails_with_the_rate_limit_code() {
    let host = MockHost::with_responses(vec![json(200, API_ERROR_261)]);
    let failure = resolver(&host)
        .check(check_request(&[
            "https://www.mediafire.com/file/ipnyzofjcwri357",
        ]))
        .await
        .expect_err("rate limited");
    assert_eq!(failure.code.as_deref(), Some("mediafire.rate_limited"));
    assert_eq!(
        failure.category,
        FailureKind::RateLimited {
            retry_after_seconds: None
        }
    );
}

/// Links that carry no key cost no request.
#[tokio::test]
async fn a_batch_without_a_single_key_makes_no_request() {
    let host = MockHost::with_responses(Vec::new());
    let results = resolver(&host)
        .check(check_request(&[
            "https://example.com/x",
            "https://www.mediafire.com/",
        ]))
        .await
        .expect("checked");
    assert!(
        results
            .iter()
            .all(|result| result.status == LinkStatus::Unknown)
    );
    assert!(host.requests().is_empty());
}

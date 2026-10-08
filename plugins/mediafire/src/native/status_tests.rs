//! `resolve` on the scripted host: how the file page's HTTP status is read, and the links that
//! are refused before any request. Split from `resolve_tests.rs` to keep both files under the
//! crate layout's 500 lines.

use rd_plugin_api::Resolver;
use rd_plugin_types::FailureKind;

use super::{FILE_PAGE, FILE_URL, GET_INFO, MockHost, html, json, resolve_request, resolver};

/// The page's status is classified the way every plugin classifies one (RA-PLG-03): a 404 or
/// 410 is final, a 451 offline and retried, a 429 waits what its `Retry-After` says, and a 403
/// is no refused account - this plugin sends none.
#[tokio::test]
async fn a_page_status_follows_the_shared_mapping() {
    for (status, retry_after, expected) in [
        (404_u16, None, FailureKind::Permanent),
        (410, None, FailureKind::Permanent),
        (451, None, FailureKind::Offline),
        (403, None, FailureKind::Permanent),
        (
            429,
            Some("120"),
            FailureKind::RateLimited {
                retry_after_seconds: Some(120),
            },
        ),
        (
            503,
            Some("0"),
            FailureKind::Transient {
                retry_after_seconds: None,
            },
        ),
    ] {
        let mut refused = html(FILE_PAGE);
        refused.status = status;
        if let Some(value) = retry_after {
            refused.headers.push(rd_plugin_api::ResolvedHeader {
                name: "Retry-After".to_owned(),
                value: value.to_owned(),
            });
        }
        let host = MockHost::answering(vec![json(200, GET_INFO), refused]);
        let failure = resolver(&host)
            .resolve(resolve_request(FILE_URL))
            .await
            .expect_err("refused");
        assert_eq!(failure.category, expected, "{status}");
        assert_eq!(failure.code.as_deref(), Some("mediafire.http_error"));
    }
}

#[tokio::test]
async fn a_page_status_that_is_not_an_answer_is_reported() {
    let mut refused = html(FILE_PAGE);
    refused.status = 503;
    let host = MockHost::answering(vec![json(200, GET_INFO), refused]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("503");
    assert_eq!(failure.code.as_deref(), Some("mediafire.http_error"));
    assert_eq!(
        failure.params.get("status").map(String::as_str),
        Some("503")
    );
}

#[tokio::test]
async fn links_that_are_not_files_are_refused_without_a_request() {
    let host = MockHost::answering(Vec::new());
    let failure = resolver(&host)
        .resolve(resolve_request(
            "https://www.mediafire.com/upgrade/get_plan.php",
        ))
        .await
        .expect_err("unsupported");
    assert_eq!(failure.code.as_deref(), Some("mediafire.unsupported_link"));
    assert_eq!(failure.category, FailureKind::Unsupported);
    let failure = resolver(&host)
        .resolve(resolve_request(
            "https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx",
        ))
        .await
        .expect_err("a list");
    assert_eq!(failure.code.as_deref(), Some("mediafire.folder_not_file"));
    assert!(host.requests().is_empty());
}

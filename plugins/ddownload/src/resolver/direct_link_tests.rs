//! The one line a silent fallback leaves behind.
//!
//! The cases are `xfs_common::test_support`'s (RD-1120-10), shared with KatFile; DDownload hands
//! in its own site, the address its API answers from and a link on its own delivery host. Each
//! drives one of the ways the undocumented `file/direct_link` endpoint can come to nothing (or
//! the one way it works) and asserts that the fallback is explained exactly once, with nothing
//! that came off the wire (RD-120-13).

use xfs_common::test_support::DirectLinkCase;

use super::api::SITE;

const CASE: DirectLinkCase = DirectLinkCase {
    site: SITE,
    api_url: "https://api-v2.ddownload.com/api/file/direct_link",
    link: "https://cdn.ddownload.com/d/abc123xyz/release.rar",
};

#[tokio::test]
async fn every_failing_direct_link_attempt_is_explained_exactly_once() {
    CASE.every_failing_attempt_is_explained_exactly_once().await;
}

/// The secret half of the same assertion: whatever the attempt saw, none of it reaches the log.
#[tokio::test]
async fn the_explanation_carries_no_file_code_key_or_address() {
    CASE.the_explanation_carries_no_file_code_key_or_address()
        .await;
}

/// The success path is silent, because there is nothing to explain.
#[tokio::test]
async fn a_direct_link_that_works_writes_nothing() {
    CASE.a_direct_link_that_works_writes_nothing().await;
}

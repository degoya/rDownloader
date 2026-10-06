//! The trace of an unrecognized session page, and the canary that proves its body stays out.
//!
//! The cases are `xfs_common::test_support`'s (RD-1120-10), shared with ddownload and KatFile;
//! FileJoker hands in its own name, the page it asks about the session and its code, and the
//! checks run against its own `check_account`. The canary page is not a recording: no FileJoker
//! page has been measured at all, signed in or not. FileJoker has no API key, so the key branch
//! the other two test has no counterpart here.

use xfs_common::test_support::{TraceCase, assert_nothing_written};

use crate::messages;
use crate::resolver;

const CASE: TraceCase = TraceCase {
    provider: "FileJoker",
    page: "the homepage",
    page_url: "https://filejoker.net/",
    title: "FileJoker - Files",
    unconfirmed_code: messages::SESSION_UNCONFIRMED,
};

#[test]
fn the_line_names_title_length_and_markers_and_nothing_else() {
    CASE.the_line_names_title_length_and_markers_and_nothing_else();
}

#[test]
fn a_page_without_title_or_markers_says_so() {
    CASE.a_page_without_title_or_markers_says_so();
}

#[test]
fn the_title_is_bounded_and_carries_no_control_characters() {
    CASE.the_title_is_bounded_and_carries_no_control_characters();
}

/// The cookie session is the whole account, so an unrecognized page is reported as
/// unconfirmed — retryable, not invalid, not a pass — and traced.
#[tokio::test]
async fn a_cookie_only_check_traces_the_unconfirmed_page_without_its_body() {
    let host = CASE.cookie_only_host();
    let failure = resolver::check_account(&host, "account")
        .await
        .expect_err("an unrecognized page is no proof of a cookie-only account");
    CASE.assert_cookie_only_unconfirmed(&host, &failure);
}

/// A page that settles the question has nothing to trace.
#[tokio::test]
async fn a_recognized_page_writes_nothing() {
    let host = CASE.recognized_host();
    resolver::check_account(&host, "account")
        .await
        .expect("a signed-in page is a session");
    assert_nothing_written(&host);
}

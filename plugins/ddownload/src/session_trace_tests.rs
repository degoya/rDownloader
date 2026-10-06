//! The trace of an unrecognized account page, and the canary that proves its body stays out.
//!
//! The cases are `xfs_common::test_support`'s (RD-1120-10), shared with KatFile and FileJoker;
//! DDownload hands in its own name, the page it asks about the session and its codes, and the
//! checks run against its own `check_account`. The canary page is not a recording: no signed-in
//! DDownload account page has been measured, so it carries the reported title only.

use xfs_common::test_support::{TraceCase, assert_nothing_written};

use crate::messages;
use crate::resolver::{self, api};

const CASE: TraceCase = TraceCase {
    provider: "DDownload",
    page: "the account page",
    page_url: "https://ddownload.com/?op=my_account",
    title: "Ultimate Cloud Storage - DDownload",
    unconfirmed_code: messages::COOKIE_SESSION_UNCONFIRMED,
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

/// The cookie-only branch: nothing but the session proves the account, so an unrecognized
/// account page is reported as unconfirmed — retryable, not invalid, not a pass — and traced.
/// It is the account page, not the homepage, that is asked.
#[tokio::test]
async fn a_cookie_only_check_traces_the_unconfirmed_page_without_its_body() {
    let host = CASE.cookie_only_host();
    let failure = resolver::check_account(&host, "account")
        .await
        .expect_err("an unrecognized page is no proof of a cookie-only account");
    CASE.assert_cookie_only_unconfirmed(&host, &failure);
}

/// The `api_key` branch: the key proved the account, the check passes, and the page that
/// settled nothing is traced the same way.
#[tokio::test]
async fn a_proven_key_traces_the_unconfirmed_page_without_its_body() {
    let host = CASE.proven_key_host(
        "https://api-v2.ddownload.com/api/account/info",
        api::API_KEY_REFERENCE,
    );
    let account = resolver::check_account(&host, "account")
        .await
        .expect("an unconfirmed session does not fail an account the key has proven");
    CASE.assert_proven_key_unconfirmed(&host, &account, messages::SESSION_UNCONFIRMED.0);
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

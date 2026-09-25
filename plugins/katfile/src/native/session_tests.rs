//! What a green account test is worth for KatFile (RD-120-13).
//!
//! The same counting pattern ddownload was reported for, in both of this plugin's branches: the
//! `api_key` branch counted the cookie jar, and the cookie-only branch accepted any 2xx answer
//! as proof of a session — which an expired jar receives, because the guest homepage is served
//! with status 200. Every case here is answered from a recorded page, never from the network.

use std::sync::Arc;

use rd_core::AccountId;
use rd_plugin_api::{Resolver, ResolverHost};

use super::super::KatfileResolver;
use super::{EXPIRED_SESSION_PAGE, MockHost, SIGNED_IN_PAGE, UNREADABLE_PAGE, json, session_page};

/// Where the session probe goes. No KatFile page has been measured, signed in or not, so the
/// probe stays where it was rather than moving to an equally unmeasured account page
/// (RD-120-46).
const SESSION_PROBE_URL: &str = "https://katfile.biz/";

const ACCOUNT_INFO: &[u8] = br#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2099-01-01 00:00:00","traffic_left":"204800"}}"#;

fn account_info() -> rd_plugin_api::HostHttpResponse {
    json("https://katfile.biz/api/account/info", ACCOUNT_INFO)
}

/// A working key and a session the site has forgotten: its own code, apart from
/// `cookie_session_invalid`, because the account is not what went wrong.
#[tokio::test]
async fn an_api_key_account_with_an_expired_cookie_session_reports_it() {
    let host = MockHost::with_responses(
        vec![account_info(), session_page(EXPIRED_SESSION_PAGE)],
        true,
    );
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("an expired session must not read as a working account");
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.download_session_expired")
    );
    assert!(
        failure.message.contains("cookie session"),
        "the message must say what to replace: {}",
        failure.message
    );
    assert!(failure.params.contains_key("diagnosis"));
}

/// A page this code does not recognize settles nothing about the session, and the key has
/// already proven the account: the check passes with the API's figures and the label says the
/// session is unconfirmed (RD-120-46, the fault RD-120-44 fixed in ddownload). Until then this
/// failed the whole check.
#[tokio::test]
async fn a_proven_key_passes_when_the_page_settles_nothing() {
    let host = MockHost::with_responses(vec![account_info(), session_page(UNREADABLE_PAGE)], true);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("an unconfirmed session does not fail an account the key has proven");
    assert!(status.valid);
    assert!(status.premium, "premium comes from the API");
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.cookies(count=1)"), "{label}");
    assert!(label.contains("katfile.session_unconfirmed"), "{label}");
    assert!(
        !label.contains("plugin.account.session_active()"),
        "nothing confirmed the session, so nothing claims it: {label}"
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests[1].url.as_str(),
        SESSION_PROBE_URL,
        "the homepage: no KatFile page is measured, so none replaces it"
    );
}

/// No cookies at all: nothing to verify, no probe, and the label says so in words.
#[tokio::test]
async fn an_api_key_account_without_cookies_is_reported_without_a_probe() {
    let host = MockHost::full(vec![account_info()], true, false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("a key-only account is still an account");
    assert!(status.valid);
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.cookies(count=0)"), "{label}");
    assert!(
        !label.contains("plugin.account.session_active()"),
        "nothing was verified, so nothing claims it: {label}"
    );
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

/// The cookie-only branch, which used to believe a bare 200. The guest homepage an expired jar
/// is served carries that status, and now it is read.
#[tokio::test]
async fn a_cookie_only_account_is_refused_on_the_guest_page_a_200_carries() {
    let resolver = KatfileResolver::new(MockHost::new(session_page(EXPIRED_SESSION_PAGE), false));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("a guest page is not a session");
    assert_eq!(failure.category, rd_core::FailureKind::AccountInvalid);
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.cookie_session_invalid")
    );
}

/// And the counter-proof: the signed-in page still reports green, and says what it verified.
#[tokio::test]
async fn a_cookie_only_account_with_a_live_session_stays_green() {
    let host = MockHost::new(session_page(SIGNED_IN_PAGE), false);
    let resolver = KatfileResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let status = resolver
        .check_account(AccountId::new())
        .await
        .expect("a live session is a working account");
    assert!(status.valid);
    let label = plugin_common::native::label_summary(&status.label);
    assert!(label.contains("plugin.account.session_active()"), "{label}");
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests[0].url.as_str(), SESSION_PROBE_URL);
}

/// The unreadable page in the cookie-only branch is retryable too, for the same reason.
#[tokio::test]
async fn a_cookie_only_account_is_not_condemned_by_a_page_it_cannot_read() {
    let resolver = KatfileResolver::new(MockHost::new(session_page(UNREADABLE_PAGE), false));
    let failure = resolver
        .check_account(AccountId::new())
        .await
        .expect_err("an unreadable page confirms nothing");
    assert_eq!(
        failure.category,
        rd_core::FailureKind::Transient {
            retry_after_seconds: None
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("katfile.cookie_session_unconfirmed")
    );
}

//! `POST /api/v1/capture/click-and-load` (RD-1260-10): Click'n'Load the browser extension took
//! in place of the agent. `jk` reaches the service as the page sent it and is evaluated only in
//! the agent's sandbox; a refusal answers with a stable code and hands nothing over.

use crate::common;

use axum::http::StatusCode;
use common::{API_BEARER, CAPTURE_BEARER, post_with_bearer, test_harness};
use serde_json::json;

const ROUTE: &str = "/api/v1/capture/click-and-load";

/// The key as a page sends it, and the two links below encrypted with it as key and IV
/// (`openssl enc -aes-128-cbc -K <key> -iv <key> -base64 -A`).
const KEY: &str = "abababababababababababababababab";
const CRYPTED: &str = "zlVrLB3ZnI5CoNWBjCrER153yDfcl2/ahvumiwz73YTQ9dqkFL1ab4CEg/JH7tyfL1rr5XjbuSYBnnM0oYOMcoA4S723sfNxtSl6xvVw6MY=";
const LINKS: [&str; 2] = [
    "https://example.com/release.part1.rar",
    "https://example.com/release.part2.rar",
];

fn candidate_urls(body: &serde_json::Value) -> Vec<String> {
    body["candidates"]
        .as_array()
        .map(|candidates| {
            candidates
                .iter()
                .filter_map(|candidate| candidate["url"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The acceptance criterion's hand-over: the links reach the LinkGrabber with the package name
/// and the password the page sent, under the Click'n'Load source.
#[tokio::test]
async fn an_encrypted_hand_over_reaches_the_linkgrabber_with_package_and_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let jk = format!("function f(){{ return '{KEY}'; }}");
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({
            "crypted": CRYPTED,
            "jk": jk,
            "passwords": "secret\nsecond",
            "package": "Release",
            "source": "https://hoster.test/folder/1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(candidate_urls(&body), LINKS);
    assert_eq!(body["batch"]["source"], "click_and_load");
    assert_eq!(body["batch"]["source_label"], "hoster.test");
    assert_eq!(body["packages"][0]["name"], "Release");
    let packages = harness
        .database
        .list_collector_packages()
        .await
        .expect("packages");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].password.as_deref(), Some("secret"));
}

#[tokio::test]
async fn links_in_the_clear_are_handed_over_too() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "urls": LINKS.join("\r\n"), "package": "Plain" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(candidate_urls(&body), LINKS);
    assert_eq!(body["batch"]["source_label"], "Browser");
}

/// A `jk` the sandbox refuses -- a script that never returns, one that declares nothing to
/// call, one with two candidate keys -- is answered with one stable code, and nothing reaches
/// the LinkGrabber.
#[tokio::test]
async fn a_jk_the_sandbox_refuses_is_refused_with_a_stable_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for jk in [
        "function f(){ while(true){} }".to_owned(),
        "no function and no key".to_owned(),
        format!(
            "function f(){{ var id='{}'; return '{KEY}'; }}",
            "0".repeat(32)
        ),
    ] {
        let (status, body) = post_with_bearer(
            &harness.router,
            ROUTE,
            CAPTURE_BEARER,
            json!({ "crypted": CRYPTED, "jk": jk }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{jk}: {body}");
        assert_eq!(body["code"], "cnl.jk_refused", "{jk}");
    }
    assert!(
        harness
            .database
            .list_candidates()
            .await
            .expect("candidates")
            .is_empty()
    );
}

/// A payload that does not decrypt says so with a code and nothing else: no account of the
/// padding, which would be an oracle against the AES path.
#[tokio::test]
async fn a_payload_that_does_not_decrypt_says_nothing_about_why() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "crypted": "bm90IHBhZGRlZCBjb3JyZWN0bHkh", "jk": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "cnl.invalid_payload");
    assert!(!body.to_string().contains("padding"), "{body}");
}

#[tokio::test]
async fn fields_are_bounded_and_unknown_ones_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "urls": LINKS[0], "passwords": "p".repeat(1025) }),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "cnl.payload_too_large");

    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "urls": "nothing here", "package": "Empty" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "cnl.no_links");

    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "urls": LINKS[0], "source": "javascript:alert(1)" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "cnl.invalid_payload");

    // The fields are the page's; anything else the extension does not send.
    let (status, _) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "urls": LINKS[0], "source_label": "spoofed" }),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert!(
        harness
            .database
            .list_candidates()
            .await
            .expect("candidates")
            .is_empty()
    );
}

/// The extension's capture token, and only a capture token.
#[tokio::test]
async fn the_route_needs_the_capture_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for bearer in ["not-a-token", API_BEARER] {
        let (status, _) =
            post_with_bearer(&harness.router, ROUTE, bearer, json!({ "urls": LINKS[0] })).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bearer}");
    }
}

/// RD-1270-05: the extension tells the person about a Click'n'Load it took, so the intake says
/// so and neither the tab nor the agent announces it a second time.
#[tokio::test]
async fn a_hand_over_through_this_route_is_left_to_the_extension_to_announce() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let mut events = harness.database.subscribe();
    let (status, body) = post_with_bearer(
        &harness.router,
        ROUTE,
        CAPTURE_BEARER,
        json!({ "urls": LINKS.join("\n") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let intake = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("the bus stays open");
            if event.kind == rd_core::EventKind::CollectorIntake {
                return event.payload;
            }
        }
    })
    .await
    .expect("the intake is announced");
    assert_eq!(intake["source"], "click_and_load");
    assert_eq!(intake[rd_db::intake_notice::PAYLOAD_KEY], true, "{intake}");
}

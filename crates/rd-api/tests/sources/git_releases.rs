//! RD-190-13: git-release subscriptions over REST, and a release file's checksum on its way into
//! the queue.
//!
//! The adapter is tested against recorded-shape answers where it lives (`rd-subscription`), the
//! poll loop's rate-limit pause and crash case in `rd-api-core`. What is checked here is the
//! contract a client sees: the options round-trip, the token is never shown, an address nobody
//! could poll is a form error, and a SHA-256 a release declared reaches the download.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_harness, test_router};
use serde_json::{Value, json};

fn body(name: &str) -> Value {
    json!({
        "name": name,
        "url": "https://github.com/example/tool",
        "kind": "git_release",
        "mode": "review",
        "interval_seconds": 3_600,
        "backlog": { "mode": "from_now" },
        "git_release": {
            "platforms": ["linux"],
            "architectures": ["x86_64"],
            "asset_patterns": ["  *.tar.gz ", "*.tar.gz", ""],
            "prereleases": false,
            "source_archives": true
        }
    })
}

#[tokio::test]
async fn a_git_release_subscription_round_trips_its_options_and_hides_its_token() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Tool releases");
    request["api_key"] = json!("github_pat_example_never_shown");

    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["kind"], "git_release");
    assert_eq!(created["git_release"]["platforms"], json!(["linux"]));
    assert_eq!(created["git_release"]["architectures"], json!(["x86_64"]));
    // Trimmed, emptied and repeated patterns dropped.
    assert_eq!(
        created["git_release"]["asset_patterns"],
        json!(["*.tar.gz"])
    );
    assert_eq!(created["git_release"]["source_archives"], true);
    assert_eq!(created["has_secret"], true);
    assert!(
        !created
            .to_string()
            .contains("github_pat_example_never_shown"),
        "{created}"
    );

    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert!(
        !listed
            .to_string()
            .contains("github_pat_example_never_shown"),
        "{listed}"
    );
}

#[tokio::test]
async fn an_address_nobody_could_poll_is_refused_with_its_reason() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let refused = |request: Value| {
        let router = router.clone();
        async move {
            let (status, response) = post_json(&router, "/api/v1/subscriptions", request).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
            response
        }
    };

    // A self-hosted forge has to be named: nothing in an address says which software serves it.
    let mut self_hosted = body("Self-hosted");
    self_hosted["url"] = json!("https://git.example.test/group/app");
    let response = refused(self_hosted.clone()).await;
    assert_eq!(response["code"], "subscription.git_forge_unknown");
    self_hosted["git_release"]["forge"] = json!("gitlab");
    let (status, created) = post_json(&router, "/api/v1/subscriptions", self_hosted).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["git_release"]["forge"], "gitlab");

    // An owner without a repository names nothing to poll.
    let mut owner_only = body("Owner only");
    owner_only["url"] = json!("https://github.com/example");
    let response = refused(owner_only).await;
    assert_eq!(response["code"], "subscription.git_repository_invalid");

    // Release options belong to a release subscription.
    let mut feed = body("Feed");
    feed["kind"] = json!("feed");
    feed["url"] = json!("https://example.test/feed.xml");
    let response = refused(feed).await;
    assert_eq!(response["code"], "subscription.git_release_kind");

    // A quarter of an hour at least: sixty unauthenticated requests an hour are soon spent.
    let mut eager = body("Eager");
    eager["interval_seconds"] = json!(600);
    let response = refused(eager).await;
    assert_eq!(response["code"], "subscription.interval_invalid");
    assert_eq!(response["params"]["minimum"], "900");
}

/// A release file's SHA-256 — the digest GitHub states, or its line in the release's checksum
/// list — travels as a declared attribute and becomes the download's expected checksum.
#[tokio::test]
async fn a_declared_sha256_becomes_the_expected_checksum_of_the_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let sha256 = "ab".repeat(32);
    let mut declared = std::collections::BTreeMap::new();
    declared.insert("release".to_owned(), "v1.2.0".to_owned());
    declared.insert("sha256".to_owned(), sha256.to_uppercase());
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Subscription,
            source_label: Some("Tool releases".to_owned()),
            package_name: None,
            password: None,
            passwords: vec![None],
            category_id: None,
            priority: None,
            providers: vec![None],
            file_names: vec![Some("tool-1.2.0-linux-x86_64.tar.gz".to_owned())],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            urls: vec![
                "https://github.com/example/tool/releases/download/v1.2.0/tool-1.2.0-linux-x86_64.tar.gz"
                    .parse()
                    .expect("url"),
            ],
            auto_check: false,
            source_attributes: vec![declared],
        })
        .await
        .expect("batch");

    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let ids: Vec<&str> = packages
        .as_array()
        .expect("packages")
        .iter()
        .filter_map(|package| package["id"].as_str())
        .collect();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": ids, "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let downloads = downloads.as_array().expect("downloads");
    assert_eq!(downloads.len(), 1, "{downloads:?}");
    assert_eq!(downloads[0]["expected_checksum"]["algorithm"], "sha256");
    assert_eq!(downloads[0]["expected_checksum"]["value"], sha256);
}

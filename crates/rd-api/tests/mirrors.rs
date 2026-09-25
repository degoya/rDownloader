//! Links in one package that point at the same file.
//!
//! The key and the winner are unit-tested next to them; what is checked here is the part that
//! only shows end to end: that enqueueing a package with two links to the same file downloads
//! one of them, and that the other is kept rather than discarded.

mod common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_harness};
use serde_json::json;

/// Adds one LinkGrabber batch whose links are already online, so nothing waits on a check.
async fn submit(harness: &common::Harness, urls: &[&str], names: &[&str]) {
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: urls.iter().map(|_| None).collect(),
            file_names: names.iter().map(|name| Some((*name).to_owned())).collect(),
            sizes: urls.iter().map(|_| None).collect(),
            requests: urls.iter().map(|_| None).collect(),
            body_refs: urls.iter().map(|_| None).collect(),
            urls: urls.iter().map(|url| url.parse().expect("url")).collect(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
}

/// Enqueues every collector package and returns the queue's download rows.
async fn enqueue_all(harness: &common::Harness) -> serde_json::Value {
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
        json!({ "ids": ids }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    downloads
}

fn states(downloads: &serde_json::Value) -> Vec<String> {
    let mut states: Vec<String> = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .filter_map(|file| file["state"].as_str().map(str::to_owned))
        .collect();
    states.sort();
    states
}

#[tokio::test]
async fn two_links_to_the_same_file_download_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(
        &harness,
        &[
            "https://one.example/release.bin",
            "https://two.example/release.bin",
        ],
        &["release.bin", "release.bin"],
    )
    .await;

    let downloads = enqueue_all(&harness).await;

    assert_eq!(
        states(&downloads),
        ["queued", "skipped"],
        "one link runs, the other waits as its fallback: {downloads}"
    );
    let groups: Vec<&str> = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .filter_map(|file| file["mirror_group"].as_str())
        .collect();
    assert_eq!(
        groups.len(),
        2,
        "both know they belong together: {downloads}"
    );
    assert_eq!(groups[0], groups[1]);
}

#[tokio::test]
async fn different_files_are_both_downloaded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(
        &harness,
        &[
            "https://one.example/part1.rar",
            "https://one.example/part2.rar",
        ],
        &["release.part1.rar", "release.part2.rar"],
    )
    .await;

    let downloads = enqueue_all(&harness).await;

    assert_eq!(
        states(&downloads),
        ["queued", "queued"],
        "volumes of one archive belong together, they are not alternatives: {downloads}"
    );
    assert!(
        downloads
            .as_array()
            .expect("downloads")
            .iter()
            .all(|file| file["mirror_group"].is_null()),
        "{downloads}"
    );
}

#[tokio::test]
async fn the_detection_can_be_switched_off() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, settings) = get_json(&harness.router, "/api/v1/settings").await;
    let mut off = settings.clone();
    off["admin_login_disabled"] = json!(true);
    off["mirror_detection"] = json!(false);
    let (status, saved) = common::put_json(&harness.router, "/api/v1/settings", off).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    submit(
        &harness,
        &[
            "https://one.example/release.bin",
            "https://two.example/release.bin",
        ],
        &["release.bin", "release.bin"],
    )
    .await;
    let downloads = enqueue_all(&harness).await;

    assert_eq!(
        states(&downloads),
        ["queued", "queued"],
        "both links are downloaded when the detection is off: {downloads}"
    );
}

#[tokio::test]
async fn links_the_source_never_named_are_not_treated_as_the_same_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    // Two unrelated files whose addresses carry no usable name. The queue synthesises one for
    // the file system — historically `download.bin` for both — but that is not evidence they
    // are the same file, and grouping them would drop one silently while the package still
    // reported itself complete.
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None, None],
            file_names: vec![None, None],
            sizes: vec![None, None],
            requests: vec![None, None],
            body_refs: vec![None, None],
            urls: vec![
                "https://one.example/download".parse().expect("url"),
                "https://two.example/download".parse().expect("url"),
            ],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    let downloads = enqueue_all(&harness).await;

    assert_eq!(
        states(&downloads),
        ["queued", "queued"],
        "both are downloaded: {downloads}"
    );
    assert!(
        downloads
            .as_array()
            .expect("downloads")
            .iter()
            .all(|file| file["mirror_group"].is_null()),
        "{downloads}"
    );
}

#[tokio::test]
async fn a_waiting_mirror_takes_over_when_the_active_link_is_cancelled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(
        &harness,
        &[
            "https://one.example/release.bin",
            "https://two.example/release.bin",
        ],
        &["release.bin", "release.bin"],
    )
    .await;
    let downloads = enqueue_all(&harness).await;
    let active = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|file| file["state"] == "queued")
        .expect("one queued")
        .clone();
    let id = active["id"].as_str().expect("id").to_owned();

    // Cancelling is as final for this link as running out of retries; without a promotion its
    // mirror would wait for a link that is never coming back.
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/downloads/{id}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(
        states(&downloads),
        ["cancelled", "queued"],
        "the mirror took over: {downloads}"
    );
}

#[tokio::test]
async fn starting_a_waiting_mirror_by_hand_stands_the_others_down() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(
        &harness,
        &[
            "https://one.example/release.bin",
            "https://two.example/release.bin",
        ],
        &["release.bin", "release.bin"],
    )
    .await;
    let downloads = enqueue_all(&harness).await;
    let waiting = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|file| file["state"] == "skipped")
        .expect("one skipped")
        .clone();
    let id = waiting["id"].as_str().expect("id").to_owned();

    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/downloads/{id}/resume"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let chosen = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|file| file["id"] == id.as_str())
        .expect("the resumed one");
    assert_eq!(
        chosen["state"], "queued",
        "the chosen link runs: {downloads}"
    );
    assert_eq!(
        states(&downloads),
        ["queued", "skipped"],
        "and exactly one of the pair does: {downloads}"
    );
}

/// A LinkGrabber filtered to one hoster enqueues what it shows and nothing it hides.
///
/// Reported from use: the hoster facet hid the other hoster's links, and "add to the queue"
/// sent them along anyway. The view names the links it shows; the package keeps the rest.
#[tokio::test]
async fn links_a_filter_hides_stay_in_their_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(
        &harness,
        &[
            "https://one.example/part1.rar",
            "https://two.example/other.bin",
            "https://one.example/part2.rar",
        ],
        &["release.part1.rar", "other.bin", "release.part2.rar"],
    )
    .await;
    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let package_id = packages[0]["id"].as_str().expect("package").to_owned();
    let (_, candidates) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    let id_of = |host: &str| -> Vec<String> {
        candidates
            .as_array()
            .expect("candidates")
            .iter()
            .filter(|candidate| {
                candidate["url"]
                    .as_str()
                    .is_some_and(|url| url.contains(host))
            })
            .filter_map(|candidate| candidate["id"].as_str().map(str::to_owned))
            .collect()
    };
    let shown = id_of("one.example");
    let hidden = id_of("two.example");
    assert_eq!((shown.len(), hidden.len()), (2, 1));

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": [package_id], "candidate_ids": shown }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(states(&downloads), ["queued", "queued"], "{downloads}");
    let (_, left) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    let left: Vec<&serde_json::Value> = left
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|candidate| candidate["state"] != "enqueued")
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["id"], hidden[0].as_str());
    assert_eq!(left[0]["package_id"], package_id.as_str());
    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    assert_eq!(packages.as_array().map(Vec::len), Some(1), "{packages}");
}

/// Hidden hosters (RD-130-21): stored normalized, a hidden hoster's mirror becomes the group's
/// fallback rather than its chosen member, and a link of a hidden hoster that is no mirror of
/// anything stays in the LinkGrabber when the view enqueues what it shows.
#[tokio::test]
async fn a_hidden_hoster_stays_behind_but_its_mirror_goes_along_as_the_fallback() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(
        &harness,
        &[
            "https://one.example/release.bin",
            "https://two.example/release.bin",
            "https://one.example/extra.bin",
        ],
        &["release.bin", "release.bin", "extra.bin"],
    )
    .await;

    let (status, stored) = put_json(
        &harness.router,
        "/api/v1/collector/mirror-preference",
        json!({ "hidden_hosters": [" WWW.One.Example ", "one.example", ""] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored["hidden_hosters"], json!(["one.example"]), "{stored}");
    let (_, read) = get_json(&harness.router, "/api/v1/collector/mirror-preference").await;
    assert_eq!(read, stored);

    let (_, candidates) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    let candidates = candidates.as_array().expect("candidates");
    let by_url = |url: &str| -> serde_json::Value {
        candidates
            .iter()
            .find(|candidate| candidate["url"] == url)
            .cloned()
            .expect("candidate")
    };
    let hidden_mirror = by_url("https://one.example/release.bin");
    let shown_mirror = by_url("https://two.example/release.bin");
    let hidden_lone = by_url("https://one.example/extra.bin");
    assert_eq!(shown_mirror["mirror"]["selected"], true, "{candidates:?}");
    assert_eq!(hidden_mirror["mirror"]["selected"], false, "{candidates:?}");

    // What the view sends: the shown group with its hidden member, not the hidden lone link.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({
            "ids": [shown_mirror["package_id"]],
            "candidate_ids": [shown_mirror["id"], hidden_mirror["id"]],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let active: Vec<&str> = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .filter(|file| file["state"] == "queued")
        .filter_map(|file| file["source"].as_str())
        .collect();
    assert_eq!(active, ["https://two.example/release.bin"], "{downloads}");
    assert_eq!(states(&downloads), ["queued", "skipped"], "{downloads}");
    let (_, left) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    let left: Vec<&serde_json::Value> = left
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|candidate| candidate["state"] != "enqueued")
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["id"], hidden_lone["id"]);

    // A name longer than any host is refused with a code the interface translates.
    let (status, problem) = put_json(
        &harness.router,
        "/api/v1/collector/mirror-preference",
        json!({ "hidden_hosters": ["x".repeat(254)] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "collector.hidden_hoster_length");
}

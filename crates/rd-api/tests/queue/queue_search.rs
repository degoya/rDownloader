//! The queue's packages and files by name (RD-1240-14), as the search palette asks for them:
//! case-insensitive, bounded, in queue order, and a nonsense query refused with its code.

use crate::common;

use axum::http::StatusCode;
use serde_json::json;

async fn queued(harness: &common::Harness, package: &str, url: &str) {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": url, "package_name": package }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

#[tokio::test]
async fn packages_and_files_are_found_by_name_and_bounded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    queued(
        &harness,
        "Ubuntu Desktop",
        "https://example.invalid/ubuntu-desktop.iso",
    )
    .await;
    queued(
        &harness,
        "Ubuntu Server",
        "https://example.invalid/ubuntu-server.iso",
    )
    .await;
    queued(&harness, "Debian", "https://example.invalid/debian.iso").await;

    let (status, found) = common::get_json(&harness.router, "/api/v1/queue/search?q=UBUNTU").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let names = found["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| package["name"].as_str().expect("name").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Ubuntu Desktop", "Ubuntu Server"]);
    let files = found["downloads"].as_array().expect("downloads");
    assert_eq!(files.len(), 2, "{found}");
    assert_eq!(files[0]["file_name"], "ubuntu-desktop.iso");
    assert_eq!(files[0]["package_name"], "Ubuntu Desktop");
    assert_eq!(files[0]["state"], "queued");

    let (_, bounded) =
        common::get_json(&harness.router, "/api/v1/queue/search?q=ubuntu&limit=1").await;
    assert_eq!(bounded["packages"].as_array().map(Vec::len), Some(1));
    assert_eq!(bounded["downloads"].as_array().map(Vec::len), Some(1));

    let (status, nothing) = common::get_json(&harness.router, "/api/v1/queue/search").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nothing, json!({ "packages": [], "downloads": [] }));

    let long = "x".repeat(201);
    for uri in [
        "/api/v1/queue/search?q=ubuntu&limit=0".to_owned(),
        "/api/v1/queue/search?q=ubuntu&limit=51".to_owned(),
        "/api/v1/queue/search?q=ubuntu&limit=many".to_owned(),
        format!("/api/v1/queue/search?q={long}"),
    ] {
        let (status, refused) = common::get_json(&harness.router, &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(refused["code"], "queue.search_invalid", "{uri}");
    }
}

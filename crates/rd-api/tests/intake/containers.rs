//! The container import endpoint, across the formats that do not need the network.
//!
//! A DLC and a CCF can only be opened by an online service, so they are covered by their
//! refusal paths in `dlc.rs`. RSDF and plain link lists are opened locally, which is what
//! makes them testable end to end.

use crate::common;

use axum::http::StatusCode;
use common::{Upload, get_json, test_router};

/// The generic container import; the file name decides the format.
const CONTAINERS: Upload<'static> = Upload {
    uri: "/api/v1/containers/import",
    file_name: "release.txt",
    part_type: "application/octet-stream",
};

#[tokio::test]
async fn a_link_list_becomes_packages_without_touching_the_network() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = common::import(&router, CONTAINERS.named("release.txt"), b"[Season 1]\nhttps://example.invalid/e01.bin\n; a comment\nhttps://example.invalid/e02.bin\n",
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["format"], "text", "{body}");
    assert_eq!(
        body["candidates"].as_array().map(Vec::len),
        Some(2),
        "{body}"
    );

    let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
    let names: Vec<&str> = packages
        .as_array()
        .expect("packages")
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    assert!(
        names.contains(&"Season 1"),
        "the heading names the package: {packages}"
    );
}

#[tokio::test]
async fn a_list_without_a_heading_is_named_after_the_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = common::import(
        &router,
        CONTAINERS.named("My Links.txt"),
        b"https://example.invalid/a.bin\n",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
    assert_eq!(packages[0]["name"], "My Links", "{packages}");
}

#[tokio::test]
async fn an_extension_the_build_does_not_read_is_refused_with_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = common::import(
        &router,
        CONTAINERS.named("notes.md"),
        b"https://example.invalid/a.bin\n",
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "container.format_unknown");
}

#[tokio::test]
async fn a_container_holding_nothing_says_so_rather_than_reporting_success() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = common::import(
        &router,
        CONTAINERS.named("empty.txt"),
        b"; only a comment\n",
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "dlc.no_links", "{body}");
}

#[tokio::test]
async fn an_rsdf_that_cannot_be_read_is_refused_rather_than_imported() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    // Valid hex and valid base64, but not this format's encryption.
    let body_hex = hex::encode("EREREREREREREREREREREQ==");
    let (status, body) = common::import(
        &router,
        CONTAINERS.named("broken.rsdf"),
        body_hex.as_bytes(),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "container.file_invalid", "{body}");
}

#[tokio::test]
async fn the_original_dlc_route_still_answers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, _) = common::import(
        &router,
        Upload {
            uri: "/api/v1/dlc/import",
            file_name: "release.dlc",
            part_type: "application/octet-stream",
        },
        b"nonsense",
    )
    .await;

    // The feature ships switched off, which the alias has to report exactly as it always did.
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

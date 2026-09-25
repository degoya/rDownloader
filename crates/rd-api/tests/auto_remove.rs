//! Automatic removal of finished packages (RD-094-01).
//!
//! The selection rules are unit-tested next to the service; what is checked here is the fact
//! they rest on — that a package records when it finished — and the bounds the setting
//! accepts, since an unbounded delay is indistinguishable from the feature being off.

mod common;

use axum::http::StatusCode;
use serde_json::json;

/// Queues one direct download and returns its package id.
async fn queued_package(router: &axum::Router) -> String {
    let (status, created) = common::post_json(
        router,
        "/api/v1/downloads",
        json!({
            "url": "https://example.invalid/movie.mkv",
            "package_name": "Example Package"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, packages) = common::get_json(router, "/api/v1/packages").await;
    packages[0]["id"].as_str().expect("package id").to_owned()
}

#[tokio::test]
async fn a_package_records_when_it_finished_and_forgets_it_when_restarted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let package_id = queued_package(&harness.router).await;
    let package_id: rd_core::PackageId = package_id.parse().expect("package id");

    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    assert!(
        packages[0]["completed_at"].is_null(),
        "a queued package has not finished: {packages}"
    );

    harness
        .database
        .set_package_state(
            package_id,
            rd_core::PackageState::Completed,
            None,
            None,
            None,
        )
        .await
        .expect("complete the package");

    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    let finished = packages[0]["completed_at"]
        .as_str()
        .expect("a finished package carries the time it finished")
        .to_owned();

    // Restarting it has to clear the stamp, or the package would be removed on the delay it
    // accrued before it was started over.
    harness
        .database
        .set_package_state(
            package_id,
            rd_core::PackageState::Downloading,
            None,
            None,
            None,
        )
        .await
        .expect("restart the package");

    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    assert!(
        packages[0]["completed_at"].is_null(),
        "the earlier finish time {finished} must not survive a restart: {packages}"
    );
}

#[tokio::test]
async fn the_removal_delay_has_to_be_a_usable_span() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, settings) = common::get_json(&router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(
        settings["auto_remove_finished"], false,
        "removing packages by itself is not something to switch on unasked: {settings}"
    );

    for hours in [0, 721] {
        let mut invalid = settings.clone();
        invalid["auto_remove_delay_hours"] = json!(hours);
        let (status, refused) = common::put_json(&router, "/api/v1/settings", invalid).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{hours} h: {refused}");
        assert_eq!(refused["code"], "settings.auto_remove_delay_invalid");
    }

    let mut accepted = settings.clone();
    accepted["auto_remove_finished"] = json!(true);
    accepted["auto_remove_delay_hours"] = json!(1);
    let (status, saved) = common::put_json(&router, "/api/v1/settings", accepted).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["auto_remove_delay_hours"], 1);
}

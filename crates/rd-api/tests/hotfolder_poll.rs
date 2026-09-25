//! RD-110-31: the hotfolder poll interval is a setting.
//!
//! Three things a person can observe: a value outside 5..=3600 is refused with a code that
//! carries the bounds, a saved value reaches the running watchers without a restart, and the
//! saved value is what the watchers are started with after a restart. The tick itself is
//! measured in `rd-hotfolder`, where the interval can be milliseconds; here the floor is five
//! seconds, so the service's interval handle stands in for the tick.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{get_json, put_json, test_harness};
use serde_json::json;

#[tokio::test]
async fn the_poll_interval_is_saved_applied_live_and_restored_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(settings["hotfolder_poll_seconds"], 30);
    assert_eq!(harness.hotfolders.poll_interval(), Duration::from_secs(30));
    // Written back as the other settings tests do it: the harness reaches its routes without
    // a password only while the administrator login stays disabled in the document.
    settings["admin_login_disabled"] = json!(true);

    for seconds in [4, 3601] {
        settings["hotfolder_poll_seconds"] = json!(seconds);
        let (status, body) = put_json(&harness.router, "/api/v1/settings", settings.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{seconds}: {body}");
        assert_eq!(body["code"], "settings.hotfolder_poll_invalid");
        assert_eq!(body["params"]["min"], "5");
        assert_eq!(body["params"]["max"], "3600");
        assert_eq!(body["params"]["seconds"], seconds.to_string());
    }
    assert_eq!(
        harness.hotfolders.poll_interval(),
        Duration::from_secs(30),
        "a refused value must not reach the watchers"
    );

    settings["hotfolder_poll_seconds"] = json!(45);
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["hotfolder_poll_seconds"], 45);
    assert_eq!(
        harness.hotfolders.poll_interval(),
        Duration::from_secs(45),
        "the running service follows the saved value without a restart"
    );

    // A restart: a second service over the same database, started the way the binary
    // starts it.
    let restarted = test_harness(directory.path()).await;
    assert_eq!(
        restarted.hotfolders.poll_interval(),
        Duration::from_secs(30)
    );
    restarted
        .hotfolders
        .start_existing()
        .await
        .expect("start existing hotfolders");
    assert_eq!(
        restarted.hotfolders.poll_interval(),
        Duration::from_secs(45)
    );
    let (_, settings) = get_json(&restarted.router, "/api/v1/settings").await;
    assert_eq!(settings["hotfolder_poll_seconds"], 45);
    restarted.hotfolders.shutdown().await;
    harness.hotfolders.shutdown().await;
}

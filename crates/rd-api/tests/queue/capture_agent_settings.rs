//! What the desktop capture agent is set to from the service (RD-1180-01, RD-1180-03): the
//! settings page and the agent read and change one row, the agent with its capture token and
//! nothing beyond it, and the shortcuts are refused with a code that names the command.

use crate::common;

use axum::http::StatusCode;
use common::{API_BEARER, CAPTURE_BEARER, get_with_bearer, post_with_bearer};

const PAGE: &str = "/api/v1/settings/capture-agent";
const AGENT: &str = "/api/v1/capture/agent-settings";
const CLIPBOARD: &str = "/api/v1/capture/clipboard";
const REPORT: &str = "/api/v1/capture/shortcut-report";

/// The tray's switch is what the settings page shows, and the page's switch is what the agent's
/// next poll reads: one row, two doors.
#[tokio::test]
async fn the_tray_and_the_settings_page_switch_the_same_clipboard_pause() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;

    let (status, fresh) = get_with_bearer(router, AGENT, CAPTURE_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    assert_eq!(
        fresh["clipboard_paused"], false,
        "watching is the default: {fresh}"
    );
    assert_eq!(fresh["shortcuts"]["send_clipboard"], "CmdOrCtrl+Alt+V");
    assert!(
        fresh["shortcuts"]["quit"].is_null(),
        "quit has no default: {fresh}"
    );

    let (status, paused) = post_with_bearer(
        router,
        CLIPBOARD,
        CAPTURE_BEARER,
        serde_json::json!({ "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["clipboard_paused"], true);
    let (status, page) = common::get_json(router, PAGE).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        page["clipboard_paused"], true,
        "the page shows the tray's switch: {page}"
    );
    assert_eq!(page["default_shortcuts"]["open"], "CmdOrCtrl+Alt+O");

    let (status, resumed) = common::patch_json(
        router,
        PAGE,
        serde_json::json!({ "clipboard_paused": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let (_, polled) = get_with_bearer(router, AGENT, CAPTURE_BEARER).await;
    assert_eq!(
        polled["clipboard_paused"], false,
        "the agent's poll reads the page's switch"
    );
}

/// The defect a field of the whole settings document would have: the page saving its shortcuts
/// switched the tray's pause back. A patch leaves what it does not name alone.
#[tokio::test]
async fn saving_the_shortcuts_leaves_the_clipboard_pause_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;

    post_with_bearer(
        router,
        CLIPBOARD,
        CAPTURE_BEARER,
        serde_json::json!({ "paused": true }),
    )
    .await;
    let (status, saved) = common::patch_json(
        router,
        PAGE,
        serde_json::json!({ "shortcuts": { "open": "option+cmdorctrl+keyr", "quit": null } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["clipboard_paused"], true, "{saved}");
    assert_eq!(
        saved["shortcuts"]["open"], "CmdOrCtrl+Alt+R",
        "stored in the canonical spelling: {saved}"
    );
    assert_eq!(
        saved["shortcuts"]["send_clipboard"], "CmdOrCtrl+Alt+V",
        "a command left out of the object takes its default: {saved}"
    );
}

#[tokio::test]
async fn a_refused_shortcut_names_its_command_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;

    for (shortcuts, code, other) in [
        (
            serde_json::json!({ "quit": "Ctrl+Alt+V" }),
            "capture.shortcut_duplicate",
            Some("send_clipboard"),
        ),
        (
            serde_json::json!({ "quit": "CmdOrCtrl+Alt+Delete" }),
            "capture.shortcut_reserved",
            None,
        ),
        (
            serde_json::json!({ "quit": "Ctrl+Q" }),
            "capture.shortcut_modifier_missing",
            None,
        ),
        (
            serde_json::json!({ "quit": "Ctrl+Alt+Nope" }),
            "capture.shortcut_invalid",
            None,
        ),
    ] {
        let (status, refused) =
            common::patch_json(router, PAGE, serde_json::json!({ "shortcuts": shortcuts })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["code"], code, "{refused}");
        assert_eq!(refused["params"]["command"], "quit", "{refused}");
        match other {
            Some(other) => assert_eq!(refused["params"]["other"], other, "{refused}"),
            None => assert!(refused["params"]["other"].is_null(), "{refused}"),
        }
    }
    let (_, page) = common::get_json(router, PAGE).await;
    assert!(
        page["shortcuts"]["quit"].is_null(),
        "nothing was stored: {page}"
    );
}

/// What the agent found when it registered the shortcuts reaches the page, dated by the service.
#[tokio::test]
async fn the_agents_shortcut_report_reaches_the_settings_page() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;

    let (_, before) = common::get_json(router, PAGE).await;
    assert!(before["report"].is_null(), "{before}");
    let (status, answered) = post_with_bearer(
        router,
        REPORT,
        CAPTURE_BEARER,
        serde_json::json!({
            "platform": "linux",
            "refused": ["pause_all", "open", "pause_all"],
            "unavailable": null,
            "reported_at": "2000-01-01T00:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{answered}");
    let (_, page) = common::get_json(router, PAGE).await;
    assert_eq!(page["report"]["platform"], "linux", "{page}");
    assert_eq!(
        page["report"]["refused"],
        serde_json::json!(["open", "pause_all"]),
        "{page}"
    );
    assert_ne!(
        page["report"]["reported_at"], "2000-01-01T00:00:00Z",
        "the service dates it, not the agent: {page}"
    );

    post_with_bearer(
        router,
        REPORT,
        CAPTURE_BEARER,
        serde_json::json!({ "platform": "linux", "unavailable": "wayland" }),
    )
    .await;
    let (_, page) = common::get_json(router, PAGE).await;
    assert_eq!(page["report"]["unavailable"], "wayland", "{page}");
}

/// The page's route is configuration; a capture token does not reach it, and an API token does
/// not reach the agent's.
#[tokio::test]
async fn each_door_takes_only_its_own_credential() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let router = &harness.router;

    let (status, refused) = get_with_bearer(router, PAGE, CAPTURE_BEARER).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
        "{status}: {refused}"
    );
    let (status, refused) = get_with_bearer(router, AGENT, API_BEARER).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
        "{status}: {refused}"
    );
    let (status, allowed) = get_with_bearer(router, PAGE, API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
}

/// The tray's switch leaves the same audit record the page's switch does, attributed to the
/// capture token that pressed it (audit 2026-10-08, API-02).
#[tokio::test]
async fn the_trays_switch_is_audited_under_its_capture_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    // With the login on, so the bearer is consulted: the default harness waves every request
    // through as anonymous before a credential is looked at.
    let harness = common::auth_harness(directory.path()).await;

    let (status, paused) = post_with_bearer(
        &harness.router,
        CLIPBOARD,
        CAPTURE_BEARER,
        serde_json::json!({ "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");

    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::SettingsChanged),
            target_id: Some("capture.agent".to_owned()),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit records");
    let record =
        serde_json::to_value(records.first().expect("the switch was not audited")).expect("json");
    assert_eq!(record["outcome"], "success", "{record}");
    assert_eq!(record["actor_kind"], "token", "{record}");
    assert_eq!(record["actor_label"], rd_core::CAPTURE_SCOPE, "{record}");
    assert_eq!(record["target_kind"], "settings", "{record}");
    assert_eq!(record["details"]["fields"], "clipboard_paused", "{record}");
}

/// RD-1210-03: the agent's poll carries where its own update stands and is answered with the
/// service's update channel; the update status shows the report beside the agent's version.
#[tokio::test]
async fn the_agents_update_report_reaches_the_update_status() {
    use tower::ServiceExt as _;

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let request = |uri: &str| {
        axum::http::Request::builder()
            .uri(uri)
            .header(axum::http::header::HOST, "127.0.0.1:8710")
            .header(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {CAPTURE_BEARER}"),
            )
            .header(axum::http::header::USER_AGENT, "rdownloader-capture/1.20.0")
    };
    let stream = harness
        .router
        .clone()
        .oneshot(
            request("/api/v1/capture/events")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream");
    assert_eq!(stream.status(), StatusCode::OK);
    let poll = harness
        .router
        .clone()
        .oneshot(
            request(AGENT)
                .header(
                    rd_update::agent::report::REPORT_HEADER,
                    "state=offered; version=1.21.0; remote=0",
                )
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("poll");
    assert_eq!(poll.status(), StatusCode::OK);
    assert_eq!(
        poll.headers()
            .get(rd_update::agent::report::CHANNEL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(rd_update::settings::default_channel(env!("CARGO_PKG_VERSION")).as_str()),
        "the agent reads the service's channel"
    );
    let (_, status) = common::get_json(&harness.router, "/api/v1/system/update").await;
    let agent = &status["capture_agents"][0];
    assert_eq!(agent["version"], "1.20.0", "{status}");
    assert_eq!(agent["self_update"], "offered", "{status}");
    assert_eq!(agent["offered_version"], "1.21.0", "{status}");
    assert_eq!(agent["remote_update_allowed"], false, "{status}");
    drop(stream);
}

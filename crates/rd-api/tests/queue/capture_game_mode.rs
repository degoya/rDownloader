//! The capture agent's game mode (RD-1240-19): the settings page sets it, the agent holds the
//! queue or a bandwidth profile for a while and lifts only its own hold, never one somebody
//! else made, changed or ended. The tray switches it on and off with the agent's own right
//! (RD-1240-23).

use crate::common;

use axum::http::StatusCode;
use common::{CAPTURE_BEARER, post_with_bearer};
use sha2::{Digest, Sha256};

const PAGE: &str = "/api/v1/settings/capture-agent";
const HOLD: &str = "/api/v1/capture/game-mode/hold";
const RELEASE: &str = "/api/v1/capture/game-mode/release";
const SWITCH: &str = "/api/v1/capture/game-mode";

/// A bearer for an agent paired with queue control.
async fn controlling_agent(database: &rd_db::Database) -> String {
    let bearer = "test-game-mode-bearer".to_owned();
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Tray".to_owned(),
            hex::encode(Sha256::digest(bearer.as_bytes())),
            vec![
                rd_core::CAPTURE_SCOPE.to_owned(),
                rd_core::CAPTURE_QUEUE_SCOPE.to_owned(),
            ],
        )
        .await
        .expect("token");
    bearer
}

async fn queued_download(router: &axum::Router) -> String {
    let (status, created) = common::post_json(
        router,
        "/api/v1/downloads",
        serde_json::json!({
            "url": "https://example.invalid/movie.mkv",
            "package_name": "Example Package"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("download id").to_owned()
}

async fn state_of(router: &axum::Router, id: &str) -> String {
    let (_, downloads) = common::get_json(router, "/api/v1/downloads").await;
    downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|download| download["id"] == id)
        .and_then(|download| download["state"].as_str())
        .expect("state")
        .to_owned()
}

async fn set_game_mode(router: &axum::Router, game_mode: serde_json::Value) -> serde_json::Value {
    let (status, page) =
        common::patch_json(router, PAGE, serde_json::json!({ "game_mode": game_mode })).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page
}

async fn hold(
    router: &axum::Router,
    bearer: &str,
    renews: &serde_json::Value,
) -> serde_json::Value {
    let (status, answer) = post_with_bearer(
        router,
        HOLD,
        bearer,
        serde_json::json!({ "renews": renews }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer
}

async fn release(router: &axum::Router, bearer: &str, until: &serde_json::Value) -> bool {
    let (status, answer) = post_with_bearer(
        router,
        RELEASE,
        bearer,
        serde_json::json!({ "until": until }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["released"].as_bool().expect("released")
}

/// Off by default; once a process is named, the agent's hold is a timed pause it renews and
/// lifts, and lifting it starts what it stopped.
#[tokio::test]
async fn the_agent_pauses_the_queue_renews_and_lifts_its_own_pause() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;
    let id = queued_download(router).await;

    let off = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(off["held"], false, "game mode is off by default: {off}");
    assert_eq!(state_of(router, &id).await, "queued");

    let page = set_game_mode(
        router,
        serde_json::json!({ "processes": [" Game.exe ", "game", ""] }),
    )
    .await;
    assert_eq!(
        page["game_mode"]["processes"],
        serde_json::json!(["Game.exe"]),
        "trimmed and once: {page}"
    );
    assert_eq!(page["game_mode"]["action"], "pause");

    let held = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(held["held"], true, "{held}");
    assert_eq!(held["action"], "pause");
    assert!(held["until"].is_string(), "{held}");
    assert_eq!(state_of(router, &id).await, "paused");
    let (_, pause) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(pause["until"], held["until"], "a timed pause: {pause}");

    let renewed = hold(router, &bearer, &held["until"]).await;
    assert_eq!(renewed["held"], true, "its own pause is renewed: {renewed}");

    // Ends are whole seconds; a renewal in the same second keeps the end.
    if held["until"] != renewed["until"] {
        assert!(
            !release(router, &bearer, &held["until"]).await,
            "an end that is no longer the pause's lifts nothing"
        );
    }
    assert!(release(router, &bearer, &renewed["until"]).await);
    assert_ne!(state_of(router, &id).await, "paused", "started again");
    let (_, pause) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(pause["paused"], false, "{pause}");
}

/// A pause somebody set is never the agent's: it does not hold over it and cannot lift it. A
/// pause of the agent's that somebody ended stays ended while the game runs.
#[tokio::test]
async fn somebody_elses_pause_is_left_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;
    let id = queued_download(router).await;
    set_game_mode(router, serde_json::json!({ "full_screen": true })).await;

    let (status, person) = common::put_json(
        router,
        "/api/v1/queue/pause",
        serde_json::json!({ "minutes": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{person}");
    let refused = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(refused["held"], false, "{refused}");
    let other_end = serde_json::json!("2030-01-01T00:00:00Z");
    assert!(!release(router, &bearer, &other_end).await);
    let (_, pause) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(pause["until"], person["until"], "untouched: {pause}");
    let (status, _) = common::delete_json(router, "/api/v1/queue/pause").await;
    assert_eq!(status, StatusCode::OK);

    let held = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(held["held"], true, "{held}");
    let (status, _) = common::delete_json(router, "/api/v1/queue/pause").await;
    assert_eq!(status, StatusCode::OK, "somebody resumed it");
    let renewal = hold(router, &bearer, &held["until"]).await;
    assert_eq!(renewal["held"], false, "it stays resumed: {renewal}");
    assert_ne!(state_of(router, &id).await, "paused");
}

/// With `action: profile` the hold is the chosen profile switched on by hand until its end, and
/// lifting it hands back to the schedule.
#[tokio::test]
async fn the_agent_switches_the_chosen_profile_and_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;
    let (status, profile) = common::post_json(
        router,
        "/api/v1/bandwidth/profiles",
        serde_json::json!({ "name": "Gaming", "download_bytes_per_second": "100000" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    set_game_mode(
        router,
        serde_json::json!({ "processes": ["game"], "action": "profile", "profile_id": profile["id"] }),
    )
    .await;

    let held = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(held["held"], true, "{held}");
    assert_eq!(held["action"], "profile");
    let (_, status) = common::get_json(router, "/api/v1/bandwidth/status").await;
    assert_eq!(status["source"], "manual", "{status}");
    assert_eq!(status["manual"]["profile_id"], profile["id"]);
    assert_eq!(status["manual"]["until"], held["until"]);

    assert!(release(router, &bearer, &held["until"]).await);
    let (_, status) = common::get_json(router, "/api/v1/bandwidth/status").await;
    assert_eq!(status["source"], "schedule", "{status}");
}

#[tokio::test]
async fn game_mode_settings_that_cannot_work_are_refused_with_a_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    for (game_mode, code) in [
        (
            serde_json::json!({ "processes": [r"C:\Games\game.exe"] }),
            "capture.game_mode_process_invalid",
        ),
        (
            serde_json::json!({ "full_screen": true, "action": "profile" }),
            "capture.game_mode_profile_missing",
        ),
        (
            serde_json::json!({
                "full_screen": true,
                "action": "profile",
                "profile_id": rd_core::BandwidthProfileId::new()
            }),
            "bandwidth.profile_not_found",
        ),
    ] {
        let (status, refused) =
            common::patch_json(router, PAGE, serde_json::json!({ "game_mode": game_mode })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["code"], code, "{refused}");
    }
    let (_, page) = common::get_json(router, PAGE).await;
    assert_eq!(
        page["game_mode"]["full_screen"], false,
        "nothing stored: {page}"
    );
}

/// The tray's switch (RD-1240-23): off keeps the programs and holds nothing, on again holds as
/// before; the settings page shows what the agent switched.
#[tokio::test]
async fn the_agent_switches_game_mode_off_and_on_and_keeps_the_rest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;
    let page = set_game_mode(router, serde_json::json!({ "processes": ["game.exe"] })).await;
    assert_eq!(
        page["game_mode"]["enabled"], true,
        "on when left out: {page}"
    );

    let (status, off) = post_with_bearer(
        router,
        SWITCH,
        &bearer,
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{off}");
    assert_eq!(off["game_mode"]["enabled"], false, "{off}");
    assert_eq!(
        off["game_mode"]["processes"],
        serde_json::json!(["game.exe"]),
        "the programs stay: {off}"
    );
    let (_, page) = common::get_json(router, PAGE).await;
    assert_eq!(page["game_mode"]["enabled"], false, "{page}");
    let refused = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(refused["held"], false, "switched off: {refused}");

    let (status, on) = post_with_bearer(
        router,
        SWITCH,
        &bearer,
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{on}");
    assert_eq!(on["game_mode"]["enabled"], true, "{on}");
    let held = hold(router, &bearer, &serde_json::Value::Null).await;
    assert_eq!(held["held"], true, "{held}");
}

/// A plain capture token may pause nothing, the game mode's hold and switch included.
#[tokio::test]
async fn an_agent_without_queue_control_cannot_hold() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    set_game_mode(router, serde_json::json!({ "full_screen": true })).await;
    for (uri, body) in [
        (HOLD, serde_json::json!({})),
        (RELEASE, serde_json::json!({ "until": chrono::Utc::now() })),
        (SWITCH, serde_json::json!({ "enabled": false })),
    ] {
        let (status, refused) = post_with_bearer(router, uri, CAPTURE_BEARER, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {refused}");
        assert_eq!(refused["code"], "auth.scope_insufficient", "{uri}");
    }
}

//! RD-120-55's tools used together with the listing tools that hand out their ids, and the
//! reads that need no id -- every answer searched for the canary by `envelope`.

use super::{API_BEARER, envelope, handshake, installation, ok, refused_with};

/// A schedule is written on a listed channel and read back, changed and removed by its listed
/// id; the automation history is read by a listed automation's id; an account's hosters by a
/// listed account's id.
#[tokio::test]
async fn schedules_automations_and_hosters_are_reached_by_listed_ids() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    ok(
        &router,
        &session,
        "create_stream_channel",
        serde_json::json!({ "definition": {
            "url": "https://twitch.tv/example", "name": "Example", "enabled": true
        }}),
    )
    .await;
    let channels = ok(
        &router,
        &session,
        "list_stream_channels",
        serde_json::json!({}),
    )
    .await;
    let channel = channels[0]["id"].as_str().expect("channel id").to_owned();
    let weekly = serde_json::json!({
        "channel_id": channel, "name": "Weekly show", "enabled": true, "kind": "weekly",
        "days": [3], "start_minute": 1200, "timezone": "Europe/Berlin", "window_minutes": 120
    });
    ok(
        &router,
        &session,
        "create_stream_schedule",
        serde_json::json!({ "definition": weekly }),
    )
    .await;
    let schedules = ok(
        &router,
        &session,
        "list_stream_schedules",
        serde_json::json!({}),
    )
    .await;
    let schedule = schedules[0]["id"].as_str().expect("schedule id").to_owned();
    let mut renamed = weekly.clone();
    renamed["name"] = "Renamed show".into();
    let updated = ok(
        &router,
        &session,
        "update_stream_schedule",
        serde_json::json!({ "id": schedule, "definition": renamed }),
    )
    .await;
    assert_eq!(updated["name"], "Renamed show", "{updated}");
    ok(
        &router,
        &session,
        "list_stream_runs",
        serde_json::json!({ "schedule_id": schedule }),
    )
    .await;
    // The planner's own rule, answered through the tool: an offset is not a zone.
    let mut offset = weekly.clone();
    offset["timezone"] = "+02:00".into();
    assert!(
        !refused_with(
            &router,
            &session,
            "create_stream_schedule",
            serde_json::json!({ "definition": offset }),
        )
        .await
        .is_empty()
    );
    ok(
        &router,
        &session,
        "delete_stream_schedule",
        serde_json::json!({ "id": schedule }),
    )
    .await;

    let vocabulary = ok(
        &router,
        &session,
        "get_automation_vocabulary",
        serde_json::json!({}),
    )
    .await;
    let trigger = vocabulary["triggers"][0].clone();
    ok(
        &router,
        &session,
        "create_automation",
        serde_json::json!({ "definition": {
            "name": "Pause on completion", "enabled": true, "trigger": trigger,
            "condition": { "type": "always" }, "actions": [{ "kind": "pause_package" }]
        }}),
    )
    .await;
    let automations = ok(&router, &session, "list_automations", serde_json::json!({})).await;
    let automation = automations[0]["automation"]["id"]
        .as_str()
        .expect("automation id")
        .to_owned();
    let versions = ok(
        &router,
        &session,
        "list_automation_versions",
        serde_json::json!({ "id": automation }),
    )
    .await;
    assert_eq!(versions.as_array().map(Vec::len), Some(1), "{versions}");
    ok(
        &router,
        &session,
        "list_automation_runs",
        serde_json::json!({ "automation_id": automation }),
    )
    .await;
    let matches = ok(
        &router,
        &session,
        "dry_run_automations",
        serde_json::json!({ "trigger": trigger }),
    )
    .await;
    assert_eq!(matches[0]["trigger_matches"], true, "{matches}");

    let accounts = ok(
        &router,
        &session,
        "list_configuration",
        serde_json::json!({ "section": "accounts" }),
    )
    .await;
    let account = accounts[0]["id"].as_str().expect("account id").to_owned();
    let hosters = ok(
        &router,
        &session,
        "list_account_hosters",
        serde_json::json!({ "id": account }),
    )
    .await;
    assert_eq!(hosters["account_id"], account.as_str(), "{hosters}");
}

/// The reads that need no id, with the data directory registered so the bundle preview is
/// answered rather than refused -- the preview reads the settings document and the log store,
/// which is where a credential would come from if the scrubbing failed.
#[tokio::test]
async fn the_remaining_reads_answer_without_a_secret() {
    let data = tempfile::tempdir().expect("tempdir");
    rd_core::set_data_directory(data.path());
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let preview = ok(
        &router,
        &session,
        "preview_diagnostic_bundle",
        serde_json::json!({}),
    )
    .await;
    assert!(
        preview["entries"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "{preview}"
    );
    for name in [
        "list_remote_job_providers",
        "get_power_status",
        "get_reconnect_status",
        "list_automation_runs",
        "list_notification_deliveries",
        "list_notification_destinations",
        "get_subscription_review_summary",
        "list_stream_schedules",
        "list_stream_runs",
    ] {
        ok(&router, &session, name, serde_json::json!({})).await;
    }
    ok(
        &router,
        &session,
        "get_plugin_messages",
        serde_json::json!({ "locale": "de" }),
    )
    .await;
    let metrics = envelope(
        &router,
        API_BEARER,
        &session,
        "get_metrics",
        &serde_json::json!({}),
    )
    .await;
    let text = metrics["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("rdownloader_"), "{metrics}");
    let cancelled = ok(
        &router,
        &session,
        "cancel_power_action",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(cancelled["code"], "power.nothing_pending", "{cancelled}");
}

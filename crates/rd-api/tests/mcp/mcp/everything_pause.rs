//! RD-190-20: the timed queue pause and the profile switched on by hand, as tools -- at the
//! price of their routes, and used together the way an agent would: list the profiles, switch
//! to one, read why it is active, hand back to the schedule; pause the queue, read the pause,
//! end it.

use super::{
    API_BEARER, CONFIG_BEARER, QUEUE_BEARER, READ_BEARER, SECRETS_BEARER, envelope, handshake,
    installation, installation_parts, ok, refused_with,
};

/// The tools and the permission their routes cost.
fn pause_tools() -> Vec<(&'static str, serde_json::Value, &'static str)> {
    use serde_json::json;
    vec![
        ("get_queue_pause", json!({}), "api:read"),
        ("pause_queue", json!({ "minutes": 5 }), "api:queue"),
        ("resume_queue", json!({}), "api:queue"),
        ("get_bandwidth_status", json!({}), "api:read"),
        ("list_bandwidth_profiles", json!({}), "api:config"),
        (
            "switch_bandwidth_profile",
            json!({ "ends": "never" }),
            "api:config",
        ),
        ("return_to_bandwidth_schedule", json!({}), "api:config"),
    ]
}

#[tokio::test]
async fn every_pause_tool_costs_what_its_route_costs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let mut sessions = Vec::new();
    for bearer in [READ_BEARER, QUEUE_BEARER, CONFIG_BEARER, SECRETS_BEARER] {
        sessions.push((bearer, handshake(&router, bearer).await));
    }
    let session = |bearer: &str| -> String {
        sessions
            .iter()
            .find(|(held, _)| *held == bearer)
            .expect("a session per token")
            .1
            .clone()
    };
    for (name, arguments, scope) in pause_tools() {
        let (short, exact) = match scope {
            "api:read" => (SECRETS_BEARER, READ_BEARER),
            "api:queue" => (CONFIG_BEARER, QUEUE_BEARER),
            "api:config" => (QUEUE_BEARER, CONFIG_BEARER),
            other => panic!("no near miss for {other}"),
        };
        let answer = envelope(&router, short, &session(short), name, &arguments).await;
        assert_eq!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{name} was not refused without {scope}: {answer}"
        );
        let answer = envelope(&router, exact, &session(exact), name, &arguments).await;
        assert!(
            answer["error"].is_null(),
            "{name} was refused with exactly {scope}: {answer}"
        );
        assert!(
            answer["result"].is_object(),
            "{name} did not reach its handler: {answer}"
        );
    }
}

#[tokio::test]
async fn an_agent_switches_a_listed_profile_and_pauses_the_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    database
        .create_bandwidth_profile(rd_db::NewBandwidthProfile {
            name: "Slow".to_owned(),
            download_bytes_per_second: rd_core::ByteCount::new(100_000).ok(),
            upload_bytes_per_second: None,
            max_active_files: None,
            daily_budget_bytes: None,
            monthly_budget_bytes: None,
            scopes: Vec::new(),
        })
        .await
        .expect("profile");
    let session = handshake(&router, API_BEARER).await;

    let profiles = ok(
        &router,
        &session,
        "list_bandwidth_profiles",
        serde_json::json!({}),
    )
    .await;
    let id = profiles[0]["id"].as_str().expect("a listed id").to_owned();
    let switched = ok(
        &router,
        &session,
        "switch_bandwidth_profile",
        serde_json::json!({ "profile_id": id, "ends": "never" }),
    )
    .await;
    assert_eq!(switched["active_profile"]["name"], "Slow", "{switched}");
    assert_eq!(switched["source"], "manual");
    let status = ok(
        &router,
        &session,
        "get_bandwidth_status",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status["source"], "manual", "{status}");
    let back = ok(
        &router,
        &session,
        "return_to_bandwidth_schedule",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(back["source"], "schedule", "{back}");
    assert_eq!(
        refused_with(
            &router,
            &session,
            "switch_bandwidth_profile",
            serde_json::json!({ "profile_id": id, "ends": "at" }),
        )
        .await,
        "bandwidth.manual_end_invalid"
    );

    let paused = ok(
        &router,
        &session,
        "pause_queue",
        serde_json::json!({ "minutes": 60 }),
    )
    .await;
    assert_eq!(paused["paused"], true, "{paused}");
    let read = ok(&router, &session, "get_queue_pause", serde_json::json!({})).await;
    assert_eq!(read["until"], paused["until"], "{read}");
    ok(&router, &session, "resume_queue", serde_json::json!({})).await;
    let read = ok(&router, &session, "get_queue_pause", serde_json::json!({})).await;
    assert_eq!(read["paused"], false, "{read}");
    assert_eq!(
        refused_with(&router, &session, "pause_queue", serde_json::json!({})).await,
        "queue.pause_end_invalid"
    );
}

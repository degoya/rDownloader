//! RD-190-20: the timed queue pause and the profile switched on by hand, as tools -- at the
//! price of their routes, and used together the way an agent would: list the profiles, switch
//! to one, read why it is active, hand back to the schedule; pause the queue, read the pause,
//! end it. RD-1210-02: the stop mark the same way -- set it, read it with the pause, clear it.

use super::{
    API_BEARER, CONFIG_BEARER, NOBODY, QUEUE_BEARER, READ_BEARER, SECRETS_BEARER, envelope,
    handshake, installation, installation_parts, ok, refused_with,
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
        (
            "set_stop_mark",
            json!({ "package_id": NOBODY }),
            "api:queue",
        ),
        ("clear_stop_mark", json!({}), "api:queue"),
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
            pause_downloads: false,
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

/// One paused file in a package of its own, written straight into the database: paused, so this
/// harness's scheduler leaves it alone, and still ahead of a stop mark.
async fn paused_file(
    database: &rd_db::Database,
    directory: &std::path::Path,
) -> rd_core::DownloadId {
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "Stop mark over MCP".to_owned(),
            destination: directory
                .join("stop-mark-mcp")
                .to_string_lossy()
                .into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: "https://example.invalid/stop-mark-mcp.bin"
                .parse()
                .expect("URL"),
            file_name: "stop-mark-mcp.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
        .id
}

#[tokio::test]
async fn an_agent_sets_reads_and_clears_the_stop_mark() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;
    let id = paused_file(&database, directory.path()).await;

    assert_eq!(
        refused_with(&router, &session, "set_stop_mark", serde_json::json!({})).await,
        "queue.stop_mark_target_invalid"
    );
    assert_eq!(
        refused_with(
            &router,
            &session,
            "set_stop_mark",
            serde_json::json!({ "package_id": NOBODY }),
        )
        .await,
        "package.not_found"
    );

    let set = ok(
        &router,
        &session,
        "set_stop_mark",
        serde_json::json!({ "download_id": id.to_string() }),
    )
    .await;
    assert_eq!(set["download_id"], id.to_string(), "{set}");
    assert_eq!(set["name"], "stop-mark-mcp.bin", "{set}");
    let read = ok(&router, &session, "get_queue_pause", serde_json::json!({})).await;
    assert_eq!(read["stop_mark"]["download_id"], id.to_string(), "{read}");
    assert_eq!(read["paused"], false, "a mark alone pauses nothing: {read}");

    let cleared = ok(&router, &session, "clear_stop_mark", serde_json::json!({})).await;
    assert_eq!(cleared["cleared"], true, "{cleared}");
    let again = ok(&router, &session, "clear_stop_mark", serde_json::json!({})).await;
    assert_eq!(again["cleared"], false, "{again}");
    let read = ok(&router, &session, "get_queue_pause", serde_json::json!({})).await;
    assert!(read["stop_mark"].is_null(), "{read}");

    // A file that is already done would stop the queue at once; the mark is refused.
    database
        .transition_download(id, rd_core::DownloadState::Cancelled)
        .await
        .expect("cancel");
    assert_eq!(
        refused_with(
            &router,
            &session,
            "set_stop_mark",
            serde_json::json!({ "download_id": id.to_string() }),
        )
        .await,
        "queue.stop_mark_target_finished"
    );
}

//! Integration tests for the torrent control surface: importing a `.torrent`, reviewing
//! its file tree, editing the selection, and how invalid edits are rejected.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn test_router(directory: &std::path::Path) -> Router {
    rd_api::router(test_state(directory).await)
}

/// The application state behind [`test_router`], for a test that builds the router itself.
async fn test_state(directory: &std::path::Path) -> rd_api::AppState {
    let database = rd_db::Database::open(directory.join("torrent-test.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let plugins = rd_plugin_host::PluginInstaller::new(
        directory.join("plugins"),
        rd_plugin_host::PluginVerifier::new(true),
    );
    let media_settings = rd_media::shared_settings(&database)
        .await
        .expect("media settings");
    let (_media_runner, media_probe) =
        rd_media::build(database.clone(), secrets.clone(), media_settings.clone());
    let gallery_settings = rd_gallery::shared_settings(&database)
        .await
        .expect("gallery settings");
    let stream_settings = rd_stream::shared_settings(&database)
        .await
        .expect("stream settings");
    let torrent_settings = rd_torrent::shared_settings(&database)
        .await
        .expect("torrent settings");
    let torrent = rd_torrent::TorrentService::start(
        database.clone(),
        torrent_settings.clone(),
        directory.to_path_buf(),
        directory.join("downloads"),
    );
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.join("passwords.txt"),
            rar_timeout: std::time::Duration::from_secs(60),
            default_scripts_directory: directory.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    let remote = rd_api::RemoteServices::new(
        database.clone(),
        secrets.clone(),
        std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
        rd_http::SharedNetworkDefaults::default(),
    );
    let state = rd_api::AppState::new(
        database,
        scheduler,
        secrets,
        plugins,
        extraction,
        media_settings,
        media_probe,
        gallery_settings,
        stream_settings,
        torrent,
        torrent_settings,
        rd_power::PowerService::default(),
        rd_core::PostprocessHold::new(),
        remote,
    );
    state.auth.set_disabled(true);
    state
}

/// Bencoded byte string.
fn bstr(value: &str) -> Vec<u8> {
    let mut out = format!("{}:", value.len()).into_bytes();
    out.extend_from_slice(value.as_bytes());
    out
}

/// A three-file torrent, enough to exercise folders and selection.
fn multi_file_torrent() -> Vec<u8> {
    let files: [(&[&str], u64); 3] = [
        (&["movie.mkv"], 4_096),
        (&["extras", "behind.nfo"], 32),
        (&["extras", "poster.jpg"], 64),
    ];
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.example/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("files"));
    bytes.push(b'l');
    for (path, length) in files {
        bytes.push(b'd');
        bytes.extend(bstr("length"));
        bytes.extend_from_slice(format!("i{length}e").as_bytes());
        bytes.extend(bstr("path"));
        bytes.push(b'l');
        for component in path {
            bytes.extend(bstr(component));
        }
        bytes.push(b'e');
        bytes.push(b'e');
    }
    bytes.push(b'e');
    bytes.extend(bstr("name"));
    bytes.extend(bstr("release"));
    bytes.extend(bstr("piece length"));
    bytes.extend_from_slice(b"i16384e");
    bytes.extend(bstr("pieces"));
    bytes.extend_from_slice(b"20:");
    bytes.extend_from_slice(&[0_u8; 20]);
    bytes.push(b'e');
    bytes.push(b'e');
    bytes
}

/// Multipart body carrying one `.torrent` file.
fn multipart(boundary: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"test.torrent\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/x-bittorrent\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

/// Imports the fixture and returns the created candidate id.
async fn import(router: &Router) -> String {
    let boundary = "rdtestboundary";
    let (status, body) = send(
        router,
        Request::post("/api/v1/torrents/import")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart(boundary, &multi_file_torrent())))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "import failed: {body}");
    body["candidates"][0]["id"]
        .as_str()
        .expect("candidate id")
        .to_owned()
}

#[tokio::test]
async fn an_imported_torrent_exposes_its_file_tree() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::get(format!("/api/v1/collector/candidates/{id}/torrent"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["metadata_state"], "ready");
    assert_eq!(body["name"], "release");
    let files = body["plan"]["files"].as_array().expect("files");
    assert_eq!(files.len(), 3);
    assert_eq!(
        files[1]["path"],
        serde_json::json!(["extras", "behind.nfo"])
    );
    // Everything is selected until the user decides otherwise.
    assert!(files.iter().all(|file| file["included"] == true));
    assert_eq!(body["plan"]["selected_bytes"], "4192");
    // The capability matrix travels with the tree so the UI can gate its controls.
    assert_eq!(body["capabilities"]["file_selection"], true);
    assert_eq!(body["capabilities"]["sequential_download"], false);
}

#[tokio::test]
async fn a_selection_is_stored_and_survives_a_reload() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"included":[0],"excluded":[1,2]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["plan"]["selected_bytes"], "4096");

    let (_, reloaded) = send(
        &router,
        Request::get(format!("/api/v1/collector/candidates/{id}/torrent"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let files = reloaded["plan"]["files"].as_array().expect("files");
    assert_eq!(files[0]["included"], true);
    assert_eq!(files[1]["included"], false);
    assert_eq!(files[2]["included"], false);
    assert_eq!(reloaded["plan"]["selected_bytes"], "4096");
}

#[tokio::test]
async fn the_candidate_list_carries_a_bounded_torrent_summary() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;
    send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"included":[0],"excluded":[1,2]}"#))
            .expect("request"),
    )
    .await;

    let (status, body) = send(
        &router,
        Request::get("/api/v1/collector/candidates")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let candidate = body
        .as_array()
        .expect("candidates")
        .iter()
        .find(|candidate| candidate["id"] == id.as_str())
        .expect("imported candidate");
    let summary = &candidate["torrent"];
    assert_eq!(summary["file_count"], 3);
    assert_eq!(summary["selected_count"], 1);
    assert_eq!(summary["metadata_state"], "ready");
    // The list must not carry the tree itself.
    assert!(summary.get("files").is_none());
}

#[tokio::test]
async fn an_unknown_file_index_is_rejected() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"included":[42],"excluded":[]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.file_index_unknown");
}

#[tokio::test]
async fn deselecting_every_file_is_rejected() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"included":[],"excluded":[0,1,2]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.plan_invalid");
}

#[tokio::test]
async fn a_file_cannot_be_included_and_excluded_at_once() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"included":[0],"excluded":[0]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.plan_invalid");
}

#[tokio::test]
async fn the_engine_capabilities_are_served() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, body) = send(
        &router,
        Request::get("/api/v1/torrents/capabilities")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "librqbit");
    assert_eq!(body["priorities_emulated"], true);
    assert_eq!(body["web_seeds"], false);
    assert_eq!(body["natpmp"], false);
}

#[tokio::test]
async fn an_exclusion_pattern_drops_only_untouched_files() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"exclusion_patterns":["*.nfo"]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let files = body["plan"]["files"].as_array().expect("files");
    assert_eq!(files[1]["included"], false);
    assert_eq!(files[1]["excluded_by_pattern"], "*.nfo");
    assert_eq!(files[0]["included"], true);
    assert_eq!(files[2]["included"], true);
}

#[tokio::test]
async fn an_explicit_decision_beats_a_matching_pattern() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"included":[1],"exclusion_patterns":["*.nfo"]}"#,
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let files = body["plan"]["files"].as_array().expect("files");
    assert_eq!(files[1]["included"], true);
    // The pattern is not reported as the reason, because it did not decide the outcome.
    assert_eq!(files[1]["excluded_by_pattern"], serde_json::Value::Null);
    assert_eq!(files[1]["explicit"], true);
}

/// Saving a pattern is what shows its effect, now that the trial-run preview is gone.
///
/// RD-108-19 removed `POST .../torrent/plan/preview`: it had no client, and the `PUT` it
/// duplicated answers with the same resolved plan, naming the pattern that dropped each
/// file. Nothing is downloading for a candidate, and clearing the field undoes it, so the
/// one round trip is the whole interaction.
#[tokio::test]
async fn saving_a_pattern_reports_which_files_it_dropped() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"exclusion_patterns":["extras/*"]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let files = body["plan"]["files"].as_array().expect("files");
    assert_eq!(files[1]["included"], false);
    assert_eq!(files[1]["excluded_by_pattern"], "extras/*");
    assert_eq!(files[2]["included"], false);

    // And clearing the field is the undo: the same endpoint, an empty pattern list.
    let (status, restored) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"exclusion_patterns":[]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    let files = restored["plan"]["files"].as_array().expect("files");
    assert!(files.iter().all(|file| file["included"] == true));
}

#[tokio::test]
async fn a_priority_is_stored_per_file() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"priorities":[{"index":0,"priority":"high"},{"index":1,"priority":"skip"}]}"#,
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let files = body["plan"]["files"].as_array().expect("files");
    assert_eq!(files[0]["priority"], "high");
    // Skip is equivalent to deselecting the file.
    assert_eq!(files[1]["priority"], "skip");
    assert_eq!(files[1]["included"], false);
    assert_eq!(files[2]["priority"], "normal");
}

#[tokio::test]
async fn sequential_mode_is_refused_by_the_engine_capability() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    for (mode, capability) in [
        ("sequential", "sequential_download"),
        ("first_last", "first_last_piece"),
    ] {
        let (status, body) = send(
            &router,
            Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"sequential":"{mode}"}}"#)))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "torrent.capability_unsupported");
        assert_eq!(body["params"]["capability"], capability);
    }
}

#[tokio::test]
async fn an_empty_or_overlong_pattern_is_rejected() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = import(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/collector/candidates/{id}/torrent/plan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"exclusion_patterns":["   "]}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.pattern_invalid");
}

/// Imports the fixture, enqueues it and returns the created download id.
async fn enqueue(router: &Router) -> String {
    let candidate = import(router).await;
    let (status, packages) = send(
        router,
        Request::get("/api/v1/collector/packages")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{packages}");
    let package = packages[0]["id"].as_str().expect("package id").to_owned();
    let (status, body) = send(
        router,
        Request::post(format!("/api/v1/collector/packages/{package}/enqueue"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "enqueue failed: {body}");
    let _ = candidate;
    let (_, downloads) = send(
        router,
        Request::get("/api/v1/downloads")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    downloads[0]["id"].as_str().expect("download id").to_owned()
}

#[tokio::test]
async fn trackers_are_listed_with_their_credentials_removed() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    // Replace the metadata tracker with one carrying a passkey.
    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/downloads/{id}/torrent/trackers"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"trackers":[{"url":"https://private.example/announce?passkey=supersecret","tier":0}]}"#,
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = body["trackers"].as_array().expect("trackers");
    assert_eq!(listed.len(), 1);
    let url = listed[0]["url"].as_str().expect("url");
    assert!(!url.contains("supersecret"), "passkey leaked: {url}");
    assert!(url.contains("private.example"));
    assert_eq!(listed[0]["origin"], "user");
    // The id is stable and carries nothing secret.
    assert!(
        !listed[0]["id"]
            .as_str()
            .expect("id")
            .contains("supersecret")
    );
}

#[tokio::test]
async fn an_edited_tracker_list_survives_a_reload() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    send(
        &router,
        Request::put(format!("/api/v1/downloads/{id}/torrent/trackers"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"trackers":[{"url":"udp://one.example:6969/announce","tier":0},{"url":"https://two.example/announce","tier":1}]}"#,
            ))
            .expect("request"),
    )
    .await;

    let (status, body) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/trackers"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = body["trackers"].as_array().expect("trackers");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["tier"], 0);
    assert_eq!(listed[1]["tier"], 1);
    assert_eq!(body["editable"], true);
}

#[tokio::test]
async fn an_unusable_tracker_scheme_is_rejected() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/downloads/{id}/torrent/trackers"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"trackers":[{"url":"ftp://tracker.example/announce"}]}"#,
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.tracker_url_invalid");
}

#[tokio::test]
async fn reannounce_is_refused_for_a_torrent_that_is_not_active() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    let (status, body) = send(
        &router,
        Request::post(format!(
            "/api/v1/downloads/{id}/torrent/trackers/reannounce"
        ))
        .body(Body::empty())
        .expect("request"),
    )
    .await;
    // Nothing is in the session in this test, so the engine cannot announce.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.not_active");
}

#[tokio::test]
async fn statistics_of_a_torrent_outside_the_session_are_not_reported_as_live() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    let (status, body) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/stats"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Nothing is in the session, so the sample must say so rather than show zeros as current.
    assert_eq!(body["live"], false);
    assert_eq!(body["ratio"], 0.0);
    assert_eq!(body["seeded_seconds"], 0);
    assert!(body["sampled_at"].is_string());
    assert!(body["session_generation"].is_number());
}

#[tokio::test]
async fn the_peer_page_is_empty_and_not_live_without_a_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    let (status, body) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/peers?limit=10"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["live"], false);
    assert_eq!(body["total"], 0);
    assert_eq!(body["peers"].as_array().expect("peers").len(), 0);
    assert_eq!(body["next_cursor"], serde_json::Value::Null);
}

#[tokio::test]
async fn piece_availability_reports_no_buckets_without_a_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    let (status, body) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/pieces"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["live"], false);
    assert_eq!(body["buckets"].as_array().expect("buckets").len(), 0);
}

#[tokio::test]
async fn full_peer_addresses_are_off_by_default() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, settings) = send(
        &router,
        Request::get("/api/v1/settings")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["torrent_peer_addresses_visible"], false);
}

#[tokio::test]
async fn the_network_status_reports_the_kill_switch_window() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, body) = send(
        &router,
        Request::get("/api/v1/torrents/network/status")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bound_interface"], serde_json::Value::Null);
    assert_eq!(body["kill_switch_enabled"], false);
    assert_eq!(body["kill_switch_engaged"], false);
    // The window a user is promised must be stated, not implied.
    assert_eq!(body["kill_switch_window_seconds"], 10);
}

#[tokio::test]
async fn the_host_interfaces_are_listed() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, body) = send(
        &router,
        Request::get("/api/v1/torrents/network/interfaces")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = body.as_array().expect("interfaces");
    assert!(!listed.is_empty());
    assert!(listed.iter().any(|entry| entry["loopback"] == true));
}

#[tokio::test]
async fn an_unknown_interface_and_a_bad_blocklist_are_rejected() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, mut settings) = send(
        &router,
        Request::get("/api/v1/settings")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    settings["torrent_bind_interface"] = serde_json::json!("rd-nonexistent-interface");
    let (status, body) = send(
        &router,
        Request::put("/api/v1/settings")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(settings.to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.interface_unknown");

    settings["torrent_bind_interface"] = serde_json::Value::Null;
    settings["torrent_ip_blocklist_url"] = serde_json::json!("file:///etc/passwd");
    let (status, body) = send(
        &router,
        Request::put("/api/v1/settings")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(settings.to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "torrent.blocklist_invalid");
}

#[tokio::test]
async fn web_seeds_are_reported_as_diagnostics_the_engine_cannot_use() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let boundary = "rdtestboundary";
    // A torrent whose `url-list` carries a web seed with a credential in the URL.
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.example/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("length"));
    bytes.extend_from_slice(b"i10e");
    bytes.extend(bstr("name"));
    bytes.extend(bstr("file.txt"));
    bytes.extend(bstr("piece length"));
    bytes.extend_from_slice(b"i16384e");
    bytes.extend(bstr("pieces"));
    bytes.extend_from_slice(b"20:");
    bytes.extend_from_slice(&[0_u8; 20]);
    bytes.push(b'e');
    bytes.extend(bstr("url-list"));
    bytes.extend(bstr("https://user:secret@seed.example/files/"));
    bytes.push(b'e');

    let (status, body) = send(
        &router,
        Request::post("/api/v1/torrents/import")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart(boundary, &bytes)))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["candidates"][0]["id"].as_str().expect("candidate id");

    let (status, detail) = send(
        &router,
        Request::get(format!("/api/v1/collector/candidates/{id}/torrent"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let seeds = detail["web_seeds"].as_array().expect("web seeds");
    assert_eq!(seeds.len(), 1);
    let seed = seeds[0].as_str().expect("seed url");
    // Shown for reference, with its credentials removed.
    assert!(seed.contains("seed.example"));
    assert!(!seed.contains("secret"));
    // And explicitly reported as something the engine will not fetch.
    assert_eq!(detail["capabilities"]["web_seeds"], false);
}

#[tokio::test]
async fn the_network_status_reports_the_proxy_and_mapping_limits() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, body) = send(
        &router,
        Request::get("/api/v1/torrents/network/status")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["peer_proxy_configured"], false);
    assert_eq!(body["upnp_enabled"], false);

    let (_, capabilities) = send(
        &router,
        Request::get("/api/v1/torrents/capabilities")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    // The proxy only covers peer connections, and only UPnP exists for mapping.
    assert_eq!(capabilities["socks5_peer_proxy"], true);
    assert_eq!(capabilities["tracker_proxy"], false);
    assert_eq!(capabilities["per_class_proxy"], false);
    assert_eq!(capabilities["upnp"], true);
    assert_eq!(capabilities["natpmp"], false);
    assert_eq!(capabilities["pcp"], false);
}

#[tokio::test]
async fn a_torrent_inherits_the_global_seeding_policy_until_it_overrides_it() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    let (status, body) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effective"]["ratio_source"], "global");
    assert_eq!(body["effective"]["enabled_source"], "global");
    assert_eq!(body["torrent_override"], serde_json::Value::Null);

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"ratio":0.25,"time_unlimited":true}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effective"]["ratio"], 0.25);
    assert_eq!(body["effective"]["ratio_source"], "torrent");
    assert_eq!(body["effective"]["time"], "unlimited");
    assert_eq!(body["effective"]["time_source"], "torrent");
    // Untouched fields keep inheriting.
    assert_eq!(body["effective"]["enabled_source"], "global");
}

#[tokio::test]
async fn a_cleared_override_makes_the_torrent_inherit_again() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    send(
        &router,
        Request::put(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"ratio":3.5}"#))
            .expect("request"),
    )
    .await;
    let (status, body) = send(
        &router,
        Request::delete(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["torrent_override"], serde_json::Value::Null);
    assert_eq!(body["effective"]["ratio_source"], "global");
}

#[tokio::test]
async fn an_out_of_range_seeding_override_is_rejected() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    for body in [r#"{"ratio":500}"#, r#"{"time_minutes":0}"#] {
        let (status, response) = send(
            &router,
            Request::put(format!("/api/v1/downloads/{id}/torrent/seeding"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(response["code"], "torrent.seeding_policy_invalid");
    }
}

#[tokio::test]
async fn a_category_override_sits_between_the_global_settings_and_the_torrent() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = enqueue(&router).await;

    // A fresh database has no categories, so the level under test has to be created.
    let root = directory.path().join("library");
    std::fs::create_dir_all(&root).expect("library directory");
    let (status, created_root) = send(
        &router,
        Request::post("/api/v1/storage-roots")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "library",
                    "path": root.to_string_lossy(),
                    "is_default": true
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created_root}");
    let (status, created_category) = send(
        &router,
        Request::post("/api/v1/categories")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "movies",
                    "color": "#336699",
                    "storage_root_id": created_root["id"],
                    "relative_path": "movies",
                    "is_default": true
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created_category}");
    let category = created_category["id"]
        .as_str()
        .expect("category id")
        .to_owned();
    // Move the queued package into it, which is what makes the category level apply.
    let (_, packages) = send(
        &router,
        Request::get("/api/v1/packages")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let package = packages[0]["id"].as_str().expect("package id").to_owned();
    let (status, moved) = send(
        &router,
        Request::post("/api/v1/packages/bulk")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "ids": [package], "category_id": category }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");

    let (status, body) = send(
        &router,
        Request::put(format!("/api/v1/categories/{category}/seeding"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"ratio":1.5,"enabled":false}"#))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, policy) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(policy["effective"]["ratio"], 1.5);
    assert_eq!(policy["effective"]["ratio_source"], "category");
    assert_eq!(policy["effective"]["enabled"], false);
    assert_eq!(policy["effective"]["enabled_source"], "category");

    // A torrent override still wins over the category.
    send(
        &router,
        Request::put(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"ratio":9.0}"#))
            .expect("request"),
    )
    .await;
    let (_, policy) = send(
        &router,
        Request::get(format!("/api/v1/downloads/{id}/torrent/seeding"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(policy["effective"]["ratio"], 9.0);
    assert_eq!(policy["effective"]["ratio_source"], "torrent");
    // …while the untouched field still comes from the category.
    assert_eq!(policy["effective"]["enabled_source"], "category");
}

// --- RD-120-68: a removed torrent stays removed ------------------------------------------

/// The info hash of the fixture torrent.
fn fixture_hash() -> String {
    rd_torrent::parse_torrent(&multi_file_torrent())
        .expect("fixture")
        .info_hash
}

/// Queues the fixture and leaves its torrent in the persisted engine session, the state a
/// restart finds. Returns the download and package ids.
async fn queued_in_session(router: &Router, directory: &std::path::Path) -> (String, String) {
    let id = enqueue(router).await;
    let (_, downloads) = send(
        router,
        Request::get("/api/v1/downloads")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let package = downloads[0]["package_id"]
        .as_str()
        .expect("package id")
        .to_owned();
    common::persist_torrent_session(directory, &fixture_hash());
    (id, package)
}

fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

/// Every removal path ends with the torrent gone from the engine session, found through the
/// info hash on the row since nothing was added in this process.
async fn assert_forgotten(router: &Router, directory: &std::path::Path) {
    let (_, downloads) = send(
        router,
        Request::get("/api/v1/downloads")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(downloads.as_array().map(Vec::len), Some(0), "{downloads}");
    assert!(
        common::persisted_torrents(directory).is_empty(),
        "the removed torrent is still in the persisted session"
    );
}

#[tokio::test]
async fn a_bulk_removal_takes_the_torrent_out_of_the_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (id, _) = queued_in_session(&router, directory.path()).await;
    assert_eq!(
        common::persisted_torrents(directory.path()),
        vec![fixture_hash()]
    );

    let (status, body) = send(
        &router,
        json_request(
            "POST",
            "/api/v1/downloads/bulk",
            serde_json::json!({ "action": "remove", "ids": [id] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 1, "{body}");
    assert_forgotten(&router, directory.path()).await;
}

#[tokio::test]
async fn removing_one_download_takes_the_torrent_out_of_the_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (id, _) = queued_in_session(&router, directory.path()).await;

    let (status, body) = send(
        &router,
        Request::delete(format!("/api/v1/downloads/{id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_forgotten(&router, directory.path()).await;
}

#[tokio::test]
async fn deleting_packages_takes_their_torrents_out_of_the_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, package) = queued_in_session(&router, directory.path()).await;

    let (status, body) = send(
        &router,
        json_request(
            "POST",
            "/api/v1/packages/delete",
            serde_json::json!({ "ids": [package], "force": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_forgotten(&router, directory.path()).await;
}

#[tokio::test]
async fn deleting_one_package_takes_its_torrent_out_of_the_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, package) = queued_in_session(&router, directory.path()).await;

    let (status, body) = send(
        &router,
        Request::delete(format!("/api/v1/packages/{package}?force=true"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_forgotten(&router, directory.path()).await;
}

/// Auto-remove passes through the same removal. Its pass runs once when the router is
/// built and then every minute, so the finished package is prepared first and a second
/// router over the same state triggers the pass.
#[tokio::test]
async fn auto_remove_takes_the_torrent_out_of_the_session() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let state = test_state(directory.path()).await;
    let router = rd_api::router(state.clone());
    let (_, package) = queued_in_session(&router, directory.path()).await;

    let (status, mut settings) = send(
        &router,
        Request::get("/api/v1/settings")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["auto_remove_finished"] = serde_json::json!(true);
    settings["auto_remove_delay_hours"] = serde_json::json!(1);
    let (status, body) = send(&router, json_request("PUT", "/api/v1/settings", settings)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Saving the settings re-reads the login switch, which this harness keeps off by hand.
    state.auth.set_disabled(true);
    // Finished two hours ago: no API writes a past finish time, so the row is aged directly.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        directory.path().join("torrent-test.sqlite3").display()
    ))
    .await
    .expect("pool");
    sqlx::query("UPDATE downloads SET state = 'completed'")
        .execute(&pool)
        .await
        .expect("complete the download");
    sqlx::query("UPDATE packages SET state = 'completed', completed_at = ? WHERE id = ?")
        .bind(chrono::Utc::now() - chrono::Duration::hours(2))
        .bind(&package)
        .execute(&pool)
        .await
        .expect("age the package");
    pool.close().await;

    // Only the pass matters here, so the second router is never asked anything.
    let database = state.database.clone();
    let _second = rd_api::router(state);
    for _ in 0..200 {
        if database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_forgotten(&router, directory.path()).await;
}

/// Serves the fixture with `content_type` at any path and counts the grabs: the GETs of
/// the whole file. The check's HEAD, and the one-byte range GET it falls back to,
/// read no torrent and are not counted (RD-130-18).
async fn torrent_server(
    content_type: &'static str,
) -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let grabs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&grabs);
    let app = Router::new().fallback(
        move |method: axum::http::Method, headers: axum::http::HeaderMap| {
            let counter = std::sync::Arc::clone(&counter);
            async move {
                if method == axum::http::Method::GET && !headers.contains_key(header::RANGE) {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                ([(header::CONTENT_TYPE, content_type)], multi_file_torrent())
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, grabs)
}

/// Adds `link` to the LinkGrabber and waits until the check has named its package after
/// the torrent. Returns the collector packages.
async fn add_rerouted(router: &Router, link: &str) -> serde_json::Value {
    let (status, body) = send(
        router,
        json_request(
            "POST",
            "/api/v1/collector/batches",
            serde_json::json!({
                "text": link,
                "source": "api",
                "source_label": null,
                "package_name": null,
                "password": null
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "intake failed: {body}");
    let mut packages = serde_json::Value::Null;
    for _ in 0..200 {
        common::wait_for_candidates_ready(router).await;
        (_, packages) = send(
            router,
            Request::get("/api/v1/collector/packages")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        if packages[0]["name"] == "release" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    packages
}

/// The owner's link: an address ending in an opaque token that answers with a `.torrent`.
/// The package is named after the torrent's `info.name`, never after the token.
#[tokio::test]
async fn a_rerouted_torrent_names_its_package_after_the_release() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (address, _) = torrent_server("application/x-bittorrent").await;
    let token = "JKs2Jt3Fo=_l3XcwZVDZXkBOYSX+nhd+A==";

    let packages = add_rerouted(&router, &format!("http://{address}/dl/{token}")).await;
    assert_eq!(packages.as_array().map(Vec::len), Some(1), "{packages}");
    assert_eq!(packages[0]["name"], "release", "{packages}");
    let (_, candidates) = send(
        &router,
        Request::get("/api/v1/collector/candidates")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(candidates[0]["provider"], "torrent", "{candidates}");
    assert_eq!(candidates[0]["file_name"], "release", "{candidates}");
    assert!(
        !packages.to_string().contains(token),
        "the token must not name anything: {packages}"
    );
}

// --- RD-130-18: a re-routed torrent is fetched once ---------------------------------------

/// Where the check keeps a candidate's torrent until it is queued.
fn kept_torrent(directory: &std::path::Path, candidate: &str) -> std::path::PathBuf {
    directory
        .join("torrents")
        .join("prefetched")
        .join(format!("{candidate}.torrent"))
}

/// The only candidate's id.
async fn only_candidate(router: &Router) -> String {
    let (_, candidates) = send(
        router,
        Request::get("/api/v1/collector/candidates")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(candidates.as_array().map(Vec::len), Some(1), "{candidates}");
    candidates[0]["id"]
        .as_str()
        .expect("candidate id")
        .to_owned()
}

/// Enqueues the first collector package and returns the source of the row it became.
async fn enqueue_source(router: &Router, packages: &serde_json::Value) -> url::Url {
    let package = packages[0]["id"].as_str().expect("package id");
    let (status, body) = send(
        router,
        json_request(
            "POST",
            &format!("/api/v1/collector/packages/{package}/enqueue"),
            serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "enqueue failed: {body}");
    let (_, downloads) = send(
        router,
        Request::get("/api/v1/downloads")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(downloads.as_array().map(Vec::len), Some(1), "{downloads}");
    downloads[0]["source"]
        .as_str()
        .expect("source")
        .parse()
        .expect("source URL")
}

/// The check reads the file once to name the package, and the row is queued from what it
/// read: a `file://` source, which the engine opens instead of asking the address again.
/// The engine is not run here — it would join a swarm — so the source is the proof that
/// the download makes no second grab, and the counter the proof that the check made one.
#[tokio::test]
async fn a_rerouted_torrent_is_grabbed_exactly_once() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (address, grabs) = torrent_server("application/x-bittorrent").await;

    let packages = add_rerouted(&router, &format!("http://{address}/dl/token")).await;
    assert_eq!(packages[0]["name"], "release", "{packages}");
    let candidate = only_candidate(&router).await;
    assert!(kept_torrent(directory.path(), &candidate).exists());
    let source = enqueue_source(&router, &packages).await;

    assert_eq!(grabs.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(source.scheme(), "file", "{source}");
    let stored = source.to_file_path().expect("stored torrent path");
    assert_eq!(
        stored.file_name().and_then(|name| name.to_str()),
        Some(format!("{}.torrent", fixture_hash()).as_str())
    );
    assert_eq!(
        std::fs::read(&stored).expect("stored torrent"),
        multi_file_torrent()
    );
    // Queued, so the kept copy has done its job.
    assert!(!kept_torrent(directory.path(), &candidate).exists());
}

/// A torrent served without its content type is recognised by its first bytes, and the
/// sniff reads that same response to its end rather than asking for the file again.
#[tokio::test]
async fn a_torrent_without_its_content_type_is_grabbed_exactly_once() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (address, grabs) = torrent_server("application/octet-stream").await;

    let packages = add_rerouted(&router, &format!("http://{address}/dl/token")).await;
    assert_eq!(packages[0]["name"], "release", "{packages}");
    let candidate = only_candidate(&router).await;
    assert!(kept_torrent(directory.path(), &candidate).exists());
    let source = enqueue_source(&router, &packages).await;

    assert_eq!(grabs.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(source.scheme(), "file", "{source}");
}

/// Past its age the kept copy is not used: the row is queued from the address, as before
/// RD-130-18, and the download fetches the file again — logged, not silent.
#[tokio::test]
async fn an_expired_torrent_is_fetched_again() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (address, _) = torrent_server("application/x-bittorrent").await;
    let link = format!("http://{address}/dl/token");

    let packages = add_rerouted(&router, &link).await;
    let kept = kept_torrent(directory.path(), &only_candidate(&router).await);
    std::fs::File::options()
        .write(true)
        .open(&kept)
        .expect("kept torrent")
        .set_modified(
            std::time::SystemTime::now()
                - rd_torrent::PREFETCH_TTL
                - std::time::Duration::from_secs(60),
        )
        .expect("age the kept torrent");
    let source = enqueue_source(&router, &packages).await;

    assert_eq!(source.as_str(), link);
    assert!(!kept.exists());
}

/// A link removed from the LinkGrabber takes its kept torrent with it.
#[tokio::test]
async fn removing_the_link_drops_the_kept_torrent() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (address, _) = torrent_server("application/x-bittorrent").await;

    add_rerouted(&router, &format!("http://{address}/dl/token")).await;
    let candidate = only_candidate(&router).await;
    let kept = kept_torrent(directory.path(), &candidate);
    assert!(kept.exists());
    let (status, body) = send(
        &router,
        Request::delete(format!("/api/v1/collector/candidates/{candidate}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!kept.exists());
}

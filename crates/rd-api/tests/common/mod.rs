//! Shared harness for the rd-api integration tests.
//!
//! Extracted rather than copied a sixth time: every test file needs the same full service
//! graph, and a per-file copy meant a change to `AppState` had to be applied five times.

#![allow(dead_code)]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// Bearer the capture routes accept in tests.
pub const CAPTURE_BEARER: &str = "test-capture-bearer-token";
/// Bearer holding the full `api:*` scope.
pub const API_BEARER: &str = "test-api-bearer-token";
/// Bearer holding the read-only `api:read` scope.
pub const READ_BEARER: &str = "test-read-bearer-token";

/// The router plus the stores behind it, for tests that reopen the database.
pub struct Harness {
    pub router: Router,
    /// Lets a test drive a batch check, and with it everything that keys off its completion.
    pub link_check: rd_api::LinkCheckService,
    pub database: rd_db::Database,
    pub secrets: rd_secrets::SecretStore,
    pub database_path: std::path::PathBuf,
    /// The daemon-side watchers, so a test can read what interval they follow (RD-110-31).
    pub hotfolders: rd_api::HotFolderService,
}

/// Builds the full router against a temporary database and secret store.
pub async fn test_router(directory: &std::path::Path) -> Router {
    test_harness(directory).await.router
}

/// Same, but keeps the database and secret store reachable.
pub async fn test_harness(directory: &std::path::Path) -> Harness {
    build_harness(directory, true, false).await
}

/// A harness whose scheduler never dispatches a queued row.
///
/// For tests that fake a download's lifecycle with `transition_download`: the live supervisor
/// claims every `queued` row within half a second, and a test racing it for the same row lost
/// on slow Windows runners (2026-09-26). With `max_active_files` at zero the dispatch loop
/// skips every job that counts against the cap; nothing in the harness raises it again.
pub async fn parked_harness(directory: &std::path::Path) -> Harness {
    build_harness(directory, true, true).await
}

/// A harness with the administrator login switched **on**.
///
/// The default harness disables it so session routes are reachable without a password;
/// scope and token tests need the opposite, because a disabled login waves every request
/// through before a scope is ever consulted.
pub async fn auth_harness(directory: &std::path::Path) -> Harness {
    build_harness(directory, false, false).await
}

async fn build_harness(directory: &std::path::Path, disable_auth: bool, parked: bool) -> Harness {
    let database_path = directory.join("api-test.sqlite3");
    let database = rd_db::Database::open(&database_path)
        .await
        .expect("database");
    // Reopening the same directory builds a second router over the same database, which is
    // how a restart is tested; the token from the first run is already there.
    for (bearer, scope) in [
        (CAPTURE_BEARER, rd_core::CAPTURE_SCOPE),
        (API_BEARER, rd_core::API_SCOPE),
        (READ_BEARER, rd_core::API_READ_SCOPE),
    ] {
        let token = database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                scope.to_owned(),
                hex::encode(Sha256::digest(bearer.as_bytes())),
                vec![scope.to_owned()],
            )
            .await;
        if let Err(error) = token {
            assert!(
                error.to_string().contains("UNIQUE constraint failed"),
                "token for {scope}: {error}"
            );
        }
    }
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
    let mut scheduler_config =
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads"));
    if parked {
        scheduler_config.max_active_files = 0;
    }
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        scheduler_config,
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
    let state = rd_api::AppState::new(
        database.clone(),
        scheduler,
        secrets.clone(),
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
        rd_api::RemoteServices::new(
            database.clone(),
            secrets.clone(),
            std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
            rd_http::SharedNetworkDefaults::default(),
        ),
    );
    // Capture intake authenticates with its own token; reading candidates back is a session
    // route, so the default stands in for an installation without an admin password.
    state.auth.set_disabled(disable_auth);
    let link_check = state.link_check.clone();
    let hotfolders = state.hotfolders.clone();
    Harness {
        router: rd_api::router(state),
        link_check,
        database,
        secrets,
        database_path,
        hotfolders,
    }
}

/// `GET` with a bearer token instead of a session.
pub async fn get_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::empty())
        .expect("request");
    send(router, request).await
}

/// Any method with a bearer token and an empty JSON body.
///
/// Used by the scope matrix, which only cares whether the request is refused before it
/// reaches a handler; the body never has to be valid for the operation.
pub async fn request_with_bearer(
    router: &Router,
    method: &str,
    uri: &str,
    bearer: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .expect("request");
    send(router, request).await
}

/// `POST` with a bearer token and a JSON body.
pub async fn post_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    send(router, request).await
}

/// `PATCH` with a bearer token and a JSON body.
pub async fn patch_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("PATCH")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    send(router, request).await
}

/// Runs one request and decodes the JSON body.
pub async fn send(router: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let payload = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, payload)
}

/// `POST /api/v1/capture/batches` with the capture bearer.
pub async fn post_capture(
    router: &Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/capture/batches")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
        .body(Body::from(body.to_string()))
        .expect("request");
    send(router, request).await
}

/// `GET` on a session route.
pub async fn get_json(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    send(router, request).await
}

/// `POST` on a session route.
pub async fn post_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    send(router, request).await
}

/// `PUT` on a session route.
pub async fn put_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(router, json_request("PUT", uri, body)).await
}

/// `PATCH` on a session route.
pub async fn patch_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(router, json_request("PATCH", uri, body)).await
}

fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

/// Waits until no candidate is still `checking` or `resolving`.
///
/// Intake starts an online check for every fresh link, and a package cannot be enqueued
/// while one is in flight.
///
/// The five seconds below are the budget for a name that **does not resolve**: the check
/// gets its refusal from the resolver and settles at once. That only holds while every
/// address a test hands in is a reserved documentation name -- `*.example`, `*.test`, or a
/// subdomain of `example.com`. The bare `example.com` is not one of those: IANA answers for
/// it, so a test using it does real resolution and a real connection attempt, and under a
/// loaded machine five seconds is not enough. That is exactly how
/// `storage_capacity::a_root_is_released_again_once_its_threshold_fits` failed on 2026-09-22,
/// passing in 3.9s alone and timing out at 9.29s inside a full run (RD-120-26).
///
/// So the limit is not the thing to raise when this panics. Look at the address the test
/// handed in first -- a longer wait would only move the coin flip.
pub async fn wait_for_candidates_ready(router: &Router) {
    for _ in 0..200 {
        let (_, candidates) = get_json(router, "/api/v1/collector/candidates").await;
        let busy = candidates.as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| matches!(item["state"].as_str(), Some("checking" | "resolving")))
        });
        if !busy {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("candidates never left the checking state");
}

/// `DELETE` on a session route.
pub async fn delete_json(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    send(router, request).await
}

/// Signs a request with a session cookie rather than a bearer.
pub async fn get_with_cookie(
    router: &Router,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::COOKIE, format!("rd_session={token}"))
        .body(Body::empty())
        .expect("request");
    send(router, request).await
}

pub async fn post_json_with_cookie(
    router: &Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::COOKIE, format!("rd_session={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    send(router, request).await
}

/// A POST whose `Set-Cookie` matters — the login, which is where a session token comes from.
pub async fn post_json_with_headers(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let token = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("rd_session="))
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json, token)
}

/// A `POST` on a session route whose own `Set-Cookie` matters.
///
/// The password change is the one route that both requires a session and hands back a new
/// one, because it ends every session including the caller's (RD-120-22). A test cannot see
/// that rotation through [`post_json_with_cookie`], which drops the response headers.
pub async fn post_json_with_cookie_and_headers(
    router: &Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::COOKIE, format!("rd_session={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let fresh = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("rd_session="))
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json, fresh)
}

pub async fn put_json_with_cookie(
    router: &Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("PUT")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::COOKIE, format!("rd_session={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    send(router, request).await
}

/// Signs in and returns the raw `Set-Cookie` value, flags and all.
pub async fn login_cookie(router: &Router, password: &str) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "password": password }).to_string(),
        ))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Writes librqbit's persisted session below a harness directory, holding one paused torrent
/// the way the engine leaves it after a restart (RD-120-68).
///
/// No session is built in these tests, so this file is the whole engine state: a removal
/// that reaches the engine strikes the entry from it.
pub fn persist_torrent_session(directory: &std::path::Path, info_hash: &str) {
    let folder = directory.join("torrent-session");
    std::fs::create_dir_all(&folder).expect("session folder");
    let session = serde_json::json!({
        "torrents": {
            "0": {
                "info_hash": info_hash,
                "trackers": [],
                "output_folder": directory.join("downloads"),
                "only_files": null,
                "is_paused": true
            }
        }
    });
    std::fs::write(
        folder.join("session.json"),
        serde_json::to_vec(&session).expect("json"),
    )
    .expect("session.json");
}

/// The info hashes librqbit's persisted session still holds.
pub fn persisted_torrents(directory: &std::path::Path) -> Vec<String> {
    let bytes = std::fs::read(directory.join("torrent-session").join("session.json"))
        .expect("session.json");
    let session: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    session["torrents"]
        .as_object()
        .map(|torrents| {
            torrents
                .values()
                .filter_map(|torrent| torrent["info_hash"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

//! Integration tests for `POST /api/v1/capture/batches`: which request metadata of an
//! intercepted browser download is accepted, which is dropped and which is rejected.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const CAPTURE_BEARER: &str = "test-capture-bearer-token";

async fn test_router(directory: &std::path::Path) -> Router {
    let database = rd_db::Database::open(directory.join("capture-test.sqlite3"))
        .await
        .expect("database");
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            rd_core::CAPTURE_SCOPE.to_owned(),
            hex::encode(Sha256::digest(CAPTURE_BEARER.as_bytes())),
            vec![rd_core::CAPTURE_SCOPE.to_owned()],
        )
        .await
        .expect("token");
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
    // Capture intake authenticates with its own token; reading candidates back is a session
    // route, so the test stands in for an installation without an admin password.
    state.auth.set_disabled(true);
    rd_api::router(state)
}

async fn post_capture(router: &Router, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/capture/batches")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
        .body(Body::from(body.to_string()))
        .expect("request");
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

/// A structured capture payload as the browser extension sends it.
fn browser_download(url: &str, headers: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "source": "browser_download",
        "source_label": "Chrome",
        "package_name": "report.pdf",
        "links": [{
            "url": url,
            "file_name": "report.pdf",
            "request": {
                "effective_url": "https://cdn.example.com/a/report.pdf",
                "method": "GET",
                "referrer": "https://example.com/downloads",
                "user_agent": "Mozilla/5.0",
                "content_disposition": "attachment; filename=\"report.pdf\"",
                "headers": headers
            }
        }]
    })
}

#[tokio::test]
async fn intercepted_download_keeps_its_request_metadata() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, payload) = post_capture(
        &router,
        browser_download(
            "https://files.example.com/report.pdf",
            serde_json::json!([{ "name": "Accept", "value": "*/*" }]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");
    let request = &payload["candidates"][0]["request"];
    assert_eq!(request["method"], "GET");
    assert_eq!(request["referrer"], "https://example.com/downloads");
    assert_eq!(request["user_agent"], "Mozilla/5.0");
    assert_eq!(
        request["content_disposition"],
        "attachment; filename=\"report.pdf\""
    );
    assert_eq!(
        request["effective_url"],
        "https://cdn.example.com/a/report.pdf"
    );
    assert_eq!(request["headers"][0]["name"], "accept");
    assert_eq!(payload["candidates"][0]["file_name"], "report.pdf");

    // The metadata is served from the database, not just echoed back. Listing candidates is a
    // session route; a fresh test database has no password, so it is open.
    let listed = Request::builder()
        .uri("/api/v1/collector/candidates")
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    let response = router.clone().oneshot(listed).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let candidates: serde_json::Value = serde_json::from_slice(&bytes).expect("candidates");
    assert_eq!(
        candidates[0]["request"]["referrer"],
        "https://example.com/downloads"
    );
}

#[tokio::test]
async fn credential_headers_are_dropped_instead_of_failing_the_handoff() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, payload) = post_capture(
        &router,
        browser_download(
            "https://files.example.com/secret.pdf",
            serde_json::json!([
                { "name": "Cookie", "value": "sid=1" },
                { "name": "Authorization", "value": "Bearer x" },
                { "name": "X-Api-Key", "value": "k" },
                { "name": "Accept-Encoding", "value": "gzip" },
                { "name": "Accept", "value": "*/*" }
            ]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");
    let headers = payload["candidates"][0]["request"]["headers"]
        .as_array()
        .expect("headers");
    assert_eq!(headers.len(), 1, "only the allowlisted header survives");
    assert_eq!(headers[0]["name"], "accept");
    let serialized = payload.to_string().to_lowercase();
    for secret in ["sid=1", "bearer x", "x-api-key"] {
        assert!(
            !serialized.contains(secret),
            "{secret} leaked into the response"
        );
    }
}

#[tokio::test]
async fn structurally_broken_metadata_is_rejected_with_stable_codes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let long = "a".repeat(rd_core::MAX_CAPTURED_VALUE + 1);
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (
            browser_download(
                "https://files.example.com/a.pdf",
                serde_json::json!([{ "name": "accept", "value": long }]),
            ),
            "capture.header_length",
        ),
        (
            browser_download(
                "https://files.example.com/b.pdf",
                serde_json::json!(vec![
                    serde_json::json!({ "name": "accept", "value": "*/*" });
                    rd_core::MAX_CAPTURED_HEADERS + 1
                ]),
            ),
            "capture.headers_limit",
        ),
        (
            serde_json::json!({
                "source": "browser_download",
                "links": [{ "url": "not a url" }]
            }),
            "collector.link_url_invalid",
        ),
        (
            // Contract v2 accepts POST; anything outside GET/POST is still structurally
            // outside the contract.
            serde_json::json!({
                "source": "browser_download",
                "links": [{
                    "url": "https://files.example.com/c.pdf",
                    "request": { "method": "PUT" }
                }]
            }),
            "capture.method_unsupported",
        ),
        (
            serde_json::json!({
                "source": "browser_download",
                "links": [{
                    "url": "https://files.example.com/d.pdf",
                    "request": { "method": "GET", "body_b64": "aWQ9MQ==" }
                }]
            }),
            "capture.body_not_allowed",
        ),
        (
            serde_json::json!({ "source": "browser_download" }),
            "collector.no_links_found",
        ),
    ];
    for (body, expected) in cases {
        let (status, payload) = post_capture(&router, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
        assert_eq!(payload["code"], expected, "{payload}");
    }
}

#[tokio::test]
async fn the_legacy_text_payload_still_works() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, payload) = post_capture(
        &router,
        serde_json::json!({
            "text": "https://files.example.com/legacy.pdf",
            "source": "browser_extension",
            "source_label": "Browser"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");
    assert_eq!(
        payload["candidates"].as_array().expect("candidates").len(),
        1
    );
    assert!(payload["candidates"][0]["request"].is_null());
}

#[tokio::test]
async fn ping_announces_the_capture_contract_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let request = Request::builder()
        .uri("/api/v1/capture/ping")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
        .body(Body::empty())
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("payload");
    assert_eq!(
        payload["capture_version"],
        rd_core::CAPTURE_CONTRACT_VERSION
    );
}

#[tokio::test]
async fn the_capture_event_stream_needs_the_capture_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let request = Request::builder()
        .uri("/api/v1/capture/events")
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the desktop agent's event stream is not open to anonymous callers"
    );
}

#[tokio::test]
async fn the_capture_event_stream_opens_for_a_paired_agent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let request = Request::builder()
        .uri("/api/v1/capture/events")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
        .body(Body::empty())
        .expect("request");
    // The body is deliberately not read: an SSE response stays open, so collecting it would
    // never return.
    let response = router.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "the agent is handed an event stream"
    );
}

/// A pasted share password comes back in no answer and goes into no log line (RD-109-32).
///
/// The address is a protected Nextcloud share with one letter wrong in the host — so no
/// crawler claims it, and RD-108-07's path, which vaults the fragment and stores the address
/// bare, never runs. Before this job the fragment survived: into `link_candidates.url`, into
/// the intake answer, into the candidate listing, and from there into every line built with
/// `rd_core::redact_url`, which leaves a fragment standing on purpose.
///
/// The three log sites that print a candidate's address — `link_check_service`,
/// `collector_enqueue` (which also writes the result into `downloads.source_path`) and
/// `replay_handlers` — are checked through the expression they use rather than through a
/// captured subscriber: what they emit is `rd_core::redact_url(&candidate.url)` of exactly the
/// address this test reads back out of the database.
#[tokio::test]
async fn a_password_in_a_fragment_comes_back_in_no_answer_and_goes_into_no_log_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, payload) = post_capture(
        &router,
        serde_json::json!({
            "text": "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret",
            "source": "browser_extension",
            "source_label": "Browser"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");
    assert_eq!(
        payload["candidates"][0]["url"], "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4",
        "the intake answer still carries the password"
    );
    assert!(
        !payload.to_string().contains("s3cret"),
        "the intake answer still carries the password: {payload}"
    );

    // Served from the database, not echoed: this is the row every viewer of the LinkGrabber
    // sees.
    let listed = Request::builder()
        .uri("/api/v1/collector/candidates")
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    let response = router.clone().oneshot(listed).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let candidates: serde_json::Value = serde_json::from_slice(&bytes).expect("candidates");
    assert!(
        !candidates.to_string().contains("s3cret"),
        "the stored candidate still carries the password: {candidates}"
    );

    let stored: url::Url = candidates[0]["url"]
        .as_str()
        .expect("an address")
        .parse()
        .expect("an address");
    assert_eq!(stored.fragment(), None);
    let logged = rd_core::redact_url(&stored);
    assert!(
        !logged.contains("s3cret"),
        "a log line built from the stored address carries the password: {logged}"
    );
}

/// RD-109-39: the refusal of an address that does not parse gives the address back nowhere.
///
/// RD-109-32 keeps a fragment out of every stored candidate, but a link that never becomes a
/// candidate went around it: the rejection was built from the pasted string itself. A mistyped
/// address of a protected share is exactly an address that fails to parse, so the likely input
/// on this path is the one carrying a password.
///
/// **The log half is the same assertion.** Nothing in `rd-api` logs a `400` by itself; what a
/// request log writes is the answer, and `ApiError::message` is the whole of its prose. An
/// answer that carries neither the password nor the address cannot put either into a log line,
/// which is why the body is checked as a whole rather than through a captured subscriber.
#[tokio::test]
async fn an_address_that_does_not_parse_comes_back_in_no_answer_and_no_log_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, payload) = post_capture(
        &router,
        serde_json::json!({
            "source": "browser_download",
            "source_label": "Chrome",
            "links": [
                { "url": "https://files.example.com/a.pdf" },
                // No scheme, so it does not parse -- and a share password rides behind the
                // hash exactly as it would on the address the user meant to paste.
                { "url": "cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
    assert_eq!(payload["code"], "collector.link_url_invalid", "{payload}");

    let answer = payload.to_string();
    assert!(
        !answer.contains("s3cret"),
        "the refusal still carries the password: {answer}"
    );
    assert!(
        !answer.contains("cloud.exmaple.org"),
        "the refusal still carries the address: {answer}"
    );
    assert!(
        !answer.contains("QxT7bK2mNp9wZr4"),
        "the refusal still carries the share token: {answer}"
    );

    // What the caller does get: which of its own links was refused. The caller holds the text
    // it just sent; what it cannot know is which one of n the service would not take.
    assert_eq!(payload["params"]["position"], "2", "{payload}");
}

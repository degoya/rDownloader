//! Pausing and resuming the whole queue from the capture agent's tray (RD-1100-06), and adding
//! everything from the LinkGrabber (RD-1240-07): only an agent paired with queue control may, a
//! plain capture token is refused with a stable code, and the summary tells the tray which of the
//! two it is.

use crate::common;

use axum::http::StatusCode;
use common::{API_BEARER, CAPTURE_BEARER, get_with_bearer, post_with_bearer};
use sha2::{Digest, Sha256};

const PAUSE: &str = "/api/v1/capture/queue/pause";
const RESUME: &str = "/api/v1/capture/queue/resume";
const SUMMARY: &str = "/api/v1/capture/summary";
const LINKGRABBER: &str = "/api/v1/capture/linkgrabber/enqueue";

/// A bearer for an agent paired with queue control.
async fn controlling_agent(database: &rd_db::Database) -> String {
    let bearer = "test-capture-queue-bearer".to_owned();
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
    let (status, downloads) = common::get_json(router, "/api/v1/downloads").await;
    assert_eq!(status, StatusCode::OK, "{downloads}");
    downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|download| download["id"] == id)
        .and_then(|download| download["state"].as_str())
        .expect("state")
        .to_owned()
}

/// The default: an agent paired without the right sees no control and may not use one. The
/// refusal is the scope policy's `403` with the scope it lacks, not the `401` of a token that
/// is no capture token at all.
#[tokio::test]
async fn a_capture_token_without_queue_control_is_refused_and_told_so() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let id = queued_download(router).await;

    let (status, summary) = get_with_bearer(router, SUMMARY, CAPTURE_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["queue_control"], false, "{summary}");

    for (uri, body) in [
        (PAUSE, serde_json::json!({})),
        (PAUSE, serde_json::json!({ "minutes": 30 })),
        (RESUME, serde_json::json!({})),
        (LINKGRABBER, serde_json::json!({ "paused": true })),
    ] {
        let (status, refused) = post_with_bearer(router, uri, CAPTURE_BEARER, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {refused}");
        assert_eq!(refused["code"], "auth.scope_insufficient", "{uri}");
        assert_eq!(refused["params"]["scope"], "capture:queue", "{uri}");
    }
    assert_eq!(state_of(router, &id).await, "queued", "nothing was paused");

    // An API token is not a capture token, whatever it may do elsewhere.
    let (status, refused) =
        post_with_bearer(router, PAUSE, API_BEARER, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    assert_eq!(refused["code"], "capture.token_required");
}

/// "Pause for 30 minutes" is the timed pause of RD-190-20, read back by the summary, and
/// "resume all" ends it.
#[tokio::test]
async fn a_paired_agent_pauses_for_a_while_and_resumes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;
    let id = queued_download(router).await;

    let (_, summary) = get_with_bearer(router, SUMMARY, &bearer).await;
    assert_eq!(summary["queue_control"], true, "{summary}");
    assert!(summary["paused_until"].is_null(), "{summary}");

    let (status, paused) =
        post_with_bearer(router, PAUSE, &bearer, serde_json::json!({ "minutes": 30 })).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["files"], 1, "{paused}");
    assert!(paused["paused_until"].is_string(), "{paused}");
    assert_eq!(state_of(router, &id).await, "paused");

    let (_, summary) = get_with_bearer(router, SUMMARY, &bearer).await;
    assert_eq!(summary["paused_until"], paused["paused_until"], "{summary}");
    assert_eq!(summary["paused"], 1, "{summary}");
    let (_, timed) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(
        timed["paused"], true,
        "the web interface sees the same pause: {timed}"
    );

    let (status, resumed) = post_with_bearer(router, RESUME, &bearer, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["files"], 1, "{resumed}");
    assert_eq!(state_of(router, &id).await, "queued");
    let (_, summary) = get_with_bearer(router, SUMMARY, &bearer).await;
    assert!(summary["paused_until"].is_null(), "{summary}");
}

/// "Pause all" without a duration stops the files until "resume all", with no end and no hold.
#[tokio::test]
async fn pausing_without_a_duration_holds_until_resumed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;
    let id = queued_download(router).await;

    let (status, paused) = post_with_bearer(router, PAUSE, &bearer, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["files"], 1, "{paused}");
    assert!(paused["paused_until"].is_null(), "{paused}");
    assert_eq!(state_of(router, &id).await, "paused");
    let (_, timed) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(timed["paused"], false, "no timed pause: {timed}");

    let (status, resumed) = post_with_bearer(router, RESUME, &bearer, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["files"], 1, "{resumed}");
    assert_eq!(state_of(router, &id).await, "queued");
}

/// The duration obeys the timed pause's own bounds, with its code.
#[tokio::test]
async fn a_duration_out_of_bounds_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let bearer = controlling_agent(&harness.database).await;
    for minutes in [0, 43_201] {
        let (status, refused) = post_with_bearer(
            &harness.router,
            PAUSE,
            &bearer,
            serde_json::json!({ "minutes": minutes }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{minutes}: {refused}");
        assert_eq!(refused["code"], "queue.pause_end_invalid", "{minutes}");
    }
}

/// Queue control is chosen when the agent is paired, and only then: a pairing that does not ask
/// for it mints exactly the token it always did.
#[tokio::test]
async fn pairing_grants_queue_control_only_when_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;

    let (status, plain) = common::post_json(
        router,
        "/api/v1/capture/pair",
        serde_json::json!({ "label": "Laptop" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{plain}");
    assert_eq!(plain["token"]["scopes"], serde_json::json!(["capture:*"]));

    let (status, controlling) = common::post_json(
        router,
        "/api/v1/capture/pair",
        serde_json::json!({ "label": "Desktop", "queue_control": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{controlling}");
    assert_eq!(
        controlling["token"]["scopes"],
        serde_json::json!(["capture:*", "capture:queue"])
    );
    let bearer = controlling["bearer"].as_str().expect("bearer");
    let (status, summary) = get_with_bearer(router, SUMMARY, bearer).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["queue_control"], true, "{summary}");
    let (status, _) = post_with_bearer(router, RESUME, bearer, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK);
}

/// "Add all from LinkGrabber" on an empty LinkGrabber is no failure: the answer is all zeros,
/// which the tray reports as an empty LinkGrabber, and nothing reaches the queue (RD-1240-07).
#[tokio::test]
async fn adding_all_from_an_empty_linkgrabber_answers_with_zeros() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let bearer = controlling_agent(&harness.database).await;

    for paused in [false, true] {
        let (status, answer) = post_with_bearer(
            router,
            LINKGRABBER,
            &bearer,
            serde_json::json!({ "paused": paused }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        for field in ["links", "nzbs", "duplicates", "failed"] {
            assert_eq!(answer[field], 0, "{field}: {answer}");
        }
        assert!(answer["first_error"].is_null(), "{answer}");
    }
    let (_, downloads) = common::get_json(router, "/api/v1/downloads").await;
    assert_eq!(downloads.as_array().map(Vec::len), Some(0), "{downloads}");
}

//! Restart recovery and the redaction boundary.
//!
//! An approved POST download has to survive a restart of the service intact, and no failure
//! may carry a signed URL into the database or onto the event stream.

mod common;

use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rd_core::{EventKind, Failure, FailureKind};

const BODY: &str = "id=42&token=s3cr3t-form-value";
const URL_SIGNATURE: &str = "abcdef0123456789deadbeef";

fn post_capture_payload() -> serde_json::Value {
    serde_json::json!({
        "source": "browser_download",
        "links": [{
            "url": "https://hoster.example/dl/42",
            "file_name": "movie.mkv",
            "request": {
                "effective_url": format!(
                    "https://cdn.example.net/f.bin?X-Amz-Signature={URL_SIGNATURE}\
                     &X-Amz-Date=20990101T000000Z&X-Amz-Expires=900"
                ),
                "method": "POST",
                "headers": [
                    { "name": "content-type", "value": "application/x-www-form-urlencoded" }
                ],
                "body_b64": STANDARD.encode(BODY.as_bytes())
            }
        }]
    })
}

#[tokio::test]
async fn an_approved_post_download_survives_a_restart_with_its_template() {
    let directory = tempfile::tempdir().expect("tempdir");
    let download_id = {
        let harness = common::test_harness(directory.path()).await;
        let (_, payload) = common::post_capture(&harness.router, post_capture_payload()).await;
        let candidate_id = payload["candidates"][0]["id"]
            .as_str()
            .expect("id")
            .to_owned();
        let package_id = payload["packages"][0]["id"]
            .as_str()
            .expect("package")
            .to_owned();

        let (_, preview) = common::get_json(
            &harness.router,
            &format!("/api/v1/collector/candidates/{candidate_id}/replay-preview"),
        )
        .await;
        let hash = preview["template_hash"].as_str().expect("hash");
        let (status, _) = common::post_json(
            &harness.router,
            &format!("/api/v1/collector/candidates/{candidate_id}/replay-consent"),
            serde_json::json!({ "template_hash": hash }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        common::wait_for_candidates_ready(&harness.router).await;
        let (status, enqueued) = common::post_json(
            &harness.router,
            &format!("/api/v1/collector/packages/{package_id}/enqueue"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{enqueued}");

        // The candidate handed its vaulted body over to the download in one transaction.
        assert!(
            harness
                .database
                .candidate_body_ref(candidate_id.parse().expect("id"))
                .await
                .expect("candidate ref")
                .is_none(),
            "the candidate must not keep a reference the download now owns"
        );
        let (_, downloads) = common::get_json(&harness.router, "/api/v1/downloads").await;
        downloads[0]["id"].as_str().expect("download id").to_owned()
    };

    // Everything above is dropped here: a fresh process reopens the same files.
    let database = rd_db::Database::open(directory.path().join("api-test.sqlite3"))
        .await
        .expect("reopen database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("reopen secrets");

    let id: rd_core::DownloadId = download_id.parse().expect("id");
    let template = database
        .request_template(id)
        .await
        .expect("template")
        .expect("a template must survive the restart");
    assert_eq!(template.request.method, "POST");
    assert!(template.request.replayable);
    assert!(!template.consent.template_hash.is_empty());
    assert_eq!(
        template.consent.approved_origins,
        ["https://hoster.example", "https://cdn.example.net"]
    );

    // The body is still decryptable, so the download can actually be started again.
    let reference = database
        .request_body_ref(id)
        .await
        .expect("body ref")
        .expect("the body reference must survive");
    let encoded = secrets.get(&reference).await.expect("decrypt");
    let bytes = STANDARD
        .decode(secrecy::ExposeSecret::expose_secret(&encoded).as_bytes())
        .expect("base64");
    assert_eq!(String::from_utf8(bytes).expect("utf-8"), BODY);

    // Nothing dangling: the body has an owner, so no sweep would remove it.
    assert!(
        database
            .orphaned_replay_body_refs()
            .await
            .expect("sweep")
            .is_empty()
    );
}

#[tokio::test]
async fn a_failure_carrying_a_signed_url_is_redacted_before_it_is_stored_or_broadcast() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (_, payload) = common::post_capture(
        &harness.router,
        serde_json::json!({
            "source": "browser_download",
            "links": [{ "url": "https://files.example.com/a.bin" }]
        }),
    )
    .await;
    let package_id = payload["packages"][0]["id"].as_str().expect("package");
    common::wait_for_candidates_ready(&harness.router).await;
    let (status, enqueued) = common::post_json(
        &harness.router,
        &format!("/api/v1/collector/packages/{package_id}/enqueue"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{enqueued}");
    let (_, downloads) = common::get_json(&harness.router, "/api/v1/downloads").await;
    let id: rd_core::DownloadId = downloads[0]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("id");

    let mut events = harness.database.subscribe();
    // Exactly what `network_failure` used to produce: the expanded URL, signature and all.
    let leaky = Failure::new(
        FailureKind::Offline,
        format!(
            "error sending request for url \
             (https://cdn.example.net/f.bin?X-Amz-Signature={URL_SIGNATURE})"
        ),
    )
    .with_param(
        "url",
        format!("https://cdn.example.net/f?sig={URL_SIGNATURE}"),
    );
    harness
        .database
        .record_failure(id, leaky, None)
        .await
        .expect("record failure");

    // The one download event that carries a Failure verbatim.
    let event = loop {
        let event = events.recv().await.expect("event");
        if event.kind == EventKind::DownloadState && event.payload.get("failure").is_some() {
            break event;
        }
    };
    let broadcast = event.payload.to_string();
    assert!(
        !broadcast.contains(URL_SIGNATURE),
        "SSE leaked: {broadcast}"
    );
    assert!(
        broadcast.contains("X-Amz-Signature"),
        "the name must survive"
    );

    // And the persisted copy the REST API serves.
    let (_, listed) = common::get_json(&harness.router, "/api/v1/downloads").await;
    assert!(
        !listed.to_string().contains(URL_SIGNATURE),
        "last_error_json leaked: {listed}"
    );
}

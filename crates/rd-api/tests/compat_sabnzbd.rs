//! The SABnzbd-compatible surface, checked the way a client actually uses it.
//!
//! These run against the auth-enabled harness, because the API key is the whole access
//! story of this adapter and a harness with the login disabled would not exercise it.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{API_BEARER, CAPTURE_BEARER, READ_BEARER, auth_harness};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// One SABnzbd call, returning the decoded body.
async fn sab(router: &axum::Router, query: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api?{query}"))
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn with_key(mode: &str) -> String {
    format!("{mode}&apikey={API_BEARER}&output=json")
}

#[tokio::test]
async fn the_adapter_answers_on_both_mount_points() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    for path in ["/api", "/sabnzbd/api"] {
        let request = Request::builder()
            .method("GET")
            .uri(format!("{path}?{}", with_key("mode=version")))
            .header(header::HOST, "127.0.0.1:8710")
            .body(Body::empty())
            .expect("request");
        let response = harness
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn a_missing_or_wrong_key_is_refused_in_sabnzbd_s_own_shape() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // A SABnzbd client reads the body and treats a non-200 as "server down", so a refusal
    // has to arrive as 200 with `status: false` or the client reports the wrong problem.
    for query in [
        "mode=version".to_owned(),
        "mode=version&apikey=".to_owned(),
        "mode=version&apikey=wrong".to_owned(),
        format!("mode=version&apikey={CAPTURE_BEARER}"),
        format!("mode=version&apikey={READ_BEARER}"),
    ] {
        let (status, body) = sab(&harness.router, &query).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(body["status"], false, "{query}: {body}");
        assert_eq!(body["error"], "API Key Incorrect", "{query}: {body}");
    }
}

#[tokio::test]
async fn version_auth_and_categories_match_the_documented_shape() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (_, version) = sab(&harness.router, &with_key("mode=version")).await;
    assert!(version["version"].is_string(), "{version}");

    let (_, auth) = sab(&harness.router, &with_key("mode=auth")).await;
    assert_eq!(auth["auth"], "apikey", "{auth}");

    let (_, cats) = sab(&harness.router, &with_key("mode=get_cats")).await;
    // SABnzbd always offers `*` as the implicit default category, and clients list it.
    assert_eq!(cats["categories"][0], "*", "{cats}");

    let (_, config) = sab(&harness.router, &with_key("mode=get_config")).await;
    assert!(
        config["config"]["misc"]["complete_dir"].is_string(),
        "a client reads complete_dir to find imported files: {config}"
    );
}

#[tokio::test]
async fn an_unknown_mode_reports_a_compatible_failure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    for mode in [
        "mode=warnings",
        "mode=",
        "mode=retry",
        "mode=queue&name=sort",
    ] {
        let (status, body) = sab(&harness.router, &with_key(mode)).await;
        assert_eq!(status, StatusCode::OK, "{mode}");
        assert_eq!(body["status"], false, "{mode}: {body}");
        assert!(
            body["error"].as_str().is_some_and(|text| !text.is_empty()),
            "{mode} answered without an error message: {body}"
        );
    }
}

#[tokio::test]
async fn an_empty_queue_and_history_carry_the_fields_a_client_reads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (_, queue) = sab(&harness.router, &with_key("mode=queue")).await;
    for field in ["slots", "status", "paused", "mb", "mbleft", "noofslots"] {
        assert!(!queue["queue"][field].is_null(), "queue.{field}: {queue}");
    }
    assert_eq!(queue["queue"]["slots"].as_array().map(Vec::len), Some(0));

    let (_, history) = sab(&harness.router, &with_key("mode=history")).await;
    for field in ["slots", "noofslots", "total_size"] {
        assert!(
            !history["history"][field].is_null(),
            "history.{field}: {history}"
        );
    }
}

#[tokio::test]
async fn an_added_nzb_appears_in_the_queue_and_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let nzo_id = add_nzb(&harness.router, "Example.Release").await;
    assert!(nzo_id.starts_with("rd_nzo_"), "{nzo_id}");

    let (_, queue) = sab(&harness.router, &with_key("mode=queue")).await;
    let slots = queue["queue"]["slots"].as_array().expect("slots");
    assert_eq!(slots.len(), 1, "{queue}");
    assert_eq!(slots[0]["nzo_id"], nzo_id, "{queue}");
    assert_eq!(slots[0]["filename"], "Example.Release", "{queue}");
    for field in ["mb", "mbleft", "percentage", "status", "cat", "priority"] {
        assert!(!slots[0][field].is_null(), "slot.{field}: {queue}");
    }

    // A second router over the same database stands in for a restart. The job id is derived
    // from the package id, so a client that stored it keeps addressing the same download.
    let restarted = auth_harness(directory.path()).await;
    let (_, queue) = sab(&restarted.router, &with_key("mode=queue")).await;
    assert_eq!(queue["queue"]["slots"][0]["nzo_id"], nzo_id, "{queue}");
}

#[tokio::test]
async fn a_queued_job_can_be_deleted_by_its_job_id() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let nzo_id = add_nzb(&harness.router, "Deletable.Release").await;
    let (_, deleted) = sab(
        &harness.router,
        &with_key(&format!("mode=queue&name=delete&value={nzo_id}")),
    )
    .await;
    assert_eq!(deleted["status"], true, "{deleted}");

    let (_, queue) = sab(&harness.router, &with_key("mode=queue")).await;
    assert_eq!(queue["queue"]["slots"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn a_foreign_job_id_is_refused_rather_than_matched() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    add_nzb(&harness.router, "Untouched.Release").await;
    for value in ["SABnzbd_nzo_abcdef", "rd_nzo_nonsense", ""] {
        let (_, body) = sab(
            &harness.router,
            &with_key(&format!("mode=queue&name=delete&value={value}")),
        )
        .await;
        assert_eq!(body["status"], false, "{value}: {body}");
    }
    let (_, queue) = sab(&harness.router, &with_key("mode=queue")).await;
    assert_eq!(
        queue["queue"]["slots"].as_array().map(Vec::len),
        Some(1),
        "a refused id removed something: {queue}"
    );
}

#[tokio::test]
async fn addurl_is_refused_with_a_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // Fetching a caller-supplied URL server-side would make the API key a request-forgery
    // primitive; clients fall back to addfile, which all of them support.
    let (status, body) = sab(
        &harness.router,
        &with_key("mode=addurl&name=http://127.0.0.1:1/x.nzb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], false, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("addfile")),
        "the refusal should name the supported path: {body}"
    );
}

/// Uploads a minimal NZB through `mode=addfile` and returns the job id it was given.
async fn add_nzb(router: &axum::Router, name: &str) -> String {
    const BOUNDARY: &str = "----rdtest";
    let nzb = format!(
        r#"<?xml version="1.0" encoding="iso-8859-1" ?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
 <file poster="poster@example.com" date="1700000000" subject="{name} [1/1] - &quot;{name}.rar&quot; yEnc (1/1)">
  <groups><group>alt.binaries.test</group></groups>
  <segments><segment bytes="1024" number="1">part1@example</segment></segments>
 </file>
</nzb>
"#
    );
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"nzbfile\"; filename=\"{name}.nzb\"\r\nContent-Type: application/x-nzb\r\n\r\n{nzb}\r\n--{BOUNDARY}--\r\n"
    );
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api?{}", with_key("mode=addfile")))
        .header(header::HOST, "127.0.0.1:8710")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(payload["status"], true, "addfile failed: {payload}");
    payload["nzo_ids"][0].as_str().expect("nzo id").to_owned()
}

/// RD-120-68: a torrent job a client deletes leaves the engine session too.
#[tokio::test]
async fn a_deleted_torrent_job_leaves_the_engine_session() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    const HASH: &str = "89abcdef0123456789abcdef0123456789abcdef";
    let (status, body) = common::post_with_bearer(
        &harness.router,
        "/api/v1/downloads",
        API_BEARER,
        serde_json::json!({ "url": format!("magnet:?xt=urn:btih:{HASH}&dn=Example.Release") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    common::persist_torrent_session(directory.path(), HASH);
    let package = harness.database.list_packages().await.expect("packages")[0].id;

    let (_, deleted) = sab(
        &harness.router,
        &with_key(&format!("mode=queue&name=delete&value=rd_nzo_{package}")),
    )
    .await;
    assert_eq!(deleted["status"], true, "{deleted}");
    assert!(
        harness
            .database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty()
    );
    assert!(common::persisted_torrents(directory.path()).is_empty());
}

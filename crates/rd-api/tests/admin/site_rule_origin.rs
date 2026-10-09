//! RD-1200-05: every rule records where it came from, and a signed rule file older than one
//! already imported from the same signer is refused (finding O-5 of the site-rules model).
//!
//! The release file is the only signed file a test can make the service accept -- its key is
//! the owner's -- so the older-sequence case raises the signer's mark above the file's own
//! sequence instead of importing a newer file first.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{get_json, post_json, put_json, send, test_harness};
use serde_json::{Value, json};

/// The signed rule file every release carries as an artifact (RD-130-07).
const RELEASE_FILE: &[u8] = include_bytes!("../../../rd-siterules/resources/site-rules.json");

/// The key the release file names and its sequence, read from the file rather than repeated.
fn release_signer() -> (String, u64) {
    let file: Value = serde_json::from_slice(RELEASE_FILE).expect("the release file");
    let signer = file["signatures"][0]["key_id"]
        .as_str()
        .expect("key id")
        .to_owned();
    let sequence = file["payload"]["sequence"].as_u64().expect("sequence");
    (signer, sequence)
}

fn origin_rule(id: &str) -> Value {
    json!({
        "id": id,
        "name": "Origin board",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["origin.example.org"], "paths": ["^/release/"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "href=\"(https?://[^\"]+)\"", "into": "links", "all": true }
        ],
        "package": { "from": "title" },
        "probe": "https://origin.example.org/release/1",
        "checked": "2026-10-08"
    })
}

fn origin_of<'a>(body: &'a Value, id: &str) -> &'a Value {
    &body["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["id"] == id)
        .unwrap_or_else(|| panic!("no rule {id} in {body}"))["origin"]
}

/// Posts a file to the import exactly as it lies on disk: the signature covers those bytes.
async fn import_bytes(router: &axum::Router, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/site-rules/import")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .expect("request");
    send(router, request).await
}

#[tokio::test]
async fn each_path_records_its_origin_and_a_switch_keeps_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (signer, sequence) = release_signer();

    let (status, body) = import_bytes(&harness.router, RELEASE_FILE.to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "format_version": 1, "rules": [origin_rule("origin-imported")] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules",
        json!({ "rule": origin_rule("origin-written") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(
        *origin_of(&body, "scnlog"),
        json!({ "kind": "signed", "signer": signer, "sequence": sequence })
    );
    assert_eq!(origin_of(&body, "origin-imported")["kind"], "import");
    assert!(origin_of(&body, "origin-imported")["signer"].is_null());
    assert_eq!(origin_of(&body, "origin-written")["kind"], "editor");

    // Switching a signed rule on changes nothing about where it came from.
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/scnlog/enabled",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(origin_of(&body, "scnlog")["kind"], "signed");

    // A changed body is the editor's: the signature covered the body it was made over.
    let mut changed = body["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["id"] == "scnlog")
        .expect("scnlog")["rule"]
        .clone();
    changed["name"] = json!("scnlog, my way");
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/scnlog",
        json!({ "rule": changed, "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(
        *origin_of(&body, "scnlog"),
        json!({ "kind": "editor", "signer": null, "sequence": null })
    );
}

/// O-5: an older file from the same signer would bring back what a newer one fixed. It is
/// refused whole, before a rule of it is stored, with a code of its own.
#[tokio::test]
async fn an_older_signed_file_is_refused_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (signer, sequence) = release_signer();
    harness
        .database
        .record_site_rule_pack(&signer, sequence + 1)
        .await
        .expect("a newer file was imported before");

    let (status, body) = import_bytes(&harness.router, RELEASE_FILE.to_vec()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "site_rules.sequence_older");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(body["rules"], json!([]), "a refused file stores nothing");
    assert_eq!(
        harness
            .database
            .record_site_rule_pack(&signer, sequence + 1)
            .await
            .expect("mark"),
        Some(sequence + 1),
        "the refused file did not lower the mark"
    );
}

//! RD-1230-03: the exchange file. What one installation exports another imports without a key
//! and without rework, every rule with the switch it had at the exporter; the preview names
//! what an import would do, and a stored rule of the same id is replaced only when asked.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{get_json, post_json, put_json, send, test_harness};
use serde_json::{Value, json};

fn rule(id: &str, host: &str) -> Value {
    json!({
        "id": id,
        "name": format!("Rule {id}"),
        "description": "One package of every link the release page lists.",
        "group": "board",
        "version": 1,
        "match": { "hosts": [host], "paths": ["^/release/"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "href=\"(https?://[^\"]+)\"", "into": "links", "all": true }
        ],
        "package": { "from": "title" },
        "probe": format!("https://{host}/release/1"),
        "checked": "2026-10-09"
    })
}

fn by_id<'a>(rules: &'a Value, id: &str) -> &'a Value {
    rules
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["id"] == id || rule["rule"]["id"] == id)
        .unwrap_or_else(|| panic!("no rule {id} in {rules}"))
}

async fn create(router: &axum::Router, body: Value, enabled: bool) {
    let (status, answer) = post_json(
        router,
        "/api/v1/site-rules",
        json!({ "rule": body, "enabled": enabled }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// Posts raw bytes to an import route, as a file read from disk arrives.
async fn post_bytes(router: &axum::Router, uri: &str, bytes: &[u8]) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes.to_vec()))
        .expect("request");
    send(router, request).await
}

/// The owner's test: an export imported on an empty installation yields the same rules, their
/// switches included, and only the switched-on ones are consulted there.
#[tokio::test]
async fn an_export_imports_on_an_empty_installation_as_the_same_rules_with_their_switches() {
    let mine = tempfile::tempdir().expect("tempdir");
    let exporter = test_harness(mine.path()).await;
    create(&exporter.router, rule("alpha", "alpha.example.org"), true).await;
    create(&exporter.router, rule("beta", "beta.example.org"), false).await;
    create(&exporter.router, rule("gamma", "gamma.example.org"), true).await;

    let (status, exported) = get_json(&exporter.router, "/api/v1/site-rules/export").await;
    assert_eq!(status, StatusCode::OK, "{exported}");
    assert_eq!(exported["format_version"], 2);
    assert_eq!(exported["rules"].as_array().map(Vec::len), Some(3));
    assert_eq!(by_id(&exported["rules"], "beta")["enabled"], false);

    // A selection exports only what it names.
    let (_, selected) = get_json(
        &exporter.router,
        "/api/v1/site-rules/export?ids=gamma,nonesuch",
    )
    .await;
    assert_eq!(selected["rules"].as_array().map(Vec::len), Some(1));
    assert_eq!(selected["rules"][0]["rule"]["id"], "gamma");

    let theirs = tempfile::tempdir().expect("tempdir");
    let colleague = test_harness(theirs.path()).await;
    let file = serde_json::to_vec(&exported).expect("file");
    let (status, preview) = post_bytes(
        &colleague.router,
        "/api/v1/site-rules/import/preview",
        &file,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    for id in ["alpha", "beta", "gamma"] {
        let entry = by_id(&preview["rules"], id);
        assert_eq!(entry["status"], "new", "{preview}");
        assert_eq!(entry["hosts"][0], format!("{id}.example.org"));
    }
    let (_, listed) = get_json(&colleague.router, "/api/v1/site-rules").await;
    assert_eq!(listed["rules"], json!([]), "the preview stores nothing");

    let (status, imported) = post_json(
        &colleague.router,
        "/api/v1/site-rules/import",
        json!({ "document": exported }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["stored"], 3);
    assert_eq!(imported["replaced"], 0);

    let (_, theirs_listed) = get_json(&colleague.router, "/api/v1/site-rules").await;
    let (_, mine_listed) = get_json(&exporter.router, "/api/v1/site-rules").await;
    for id in ["alpha", "beta", "gamma"] {
        let there = by_id(&theirs_listed["rules"], id);
        let here = by_id(&mine_listed["rules"], id);
        assert_eq!(there["rule"], here["rule"], "{id}: the same body");
        assert_eq!(there["enabled"], here["enabled"], "{id}: the same switch");
        assert_eq!(there["origin"]["kind"], "import");
        assert_eq!(
            there["description"],
            "One package of every link the release page lists."
        );
    }
    let catalogue = rd_api::site_rule_catalogue(&colleague.database).await;
    assert!(catalogue.get("alpha").is_some() && catalogue.get("gamma").is_some());
    assert!(
        catalogue.get("beta").is_none(),
        "a switched-off rule stays off"
    );
}

/// A stored rule of the same id is replaced only when the request names it; an identical one is
/// `same`, and a body that does not read or an id the file repeats is refused alone.
#[tokio::test]
async fn an_import_replaces_a_stored_rule_only_when_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    create(&harness.router, rule("kept", "kept.example.org"), false).await;
    create(&harness.router, rule("unchanged", "same.example.org"), true).await;

    let mut changed = rule("kept", "kept.example.org");
    changed["name"] = json!("Kept, as my colleague wrote it");
    let mut broken = rule("broken", "broken.example.org");
    broken["steps"] = json!([]);
    let document = json!({
        "format_version": 2,
        "rules": [
            { "enabled": true, "rule": changed },
            { "enabled": true, "rule": rule("unchanged", "same.example.org") },
            { "enabled": false, "rule": rule("fresh", "fresh.example.org") },
            { "enabled": false, "rule": broken },
            { "enabled": false, "rule": rule("fresh", "fresh.example.org") },
        ]
    });

    let (status, preview) = post_json(
        &harness.router,
        "/api/v1/site-rules/import/preview",
        document.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let words: Vec<&str> = preview["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .filter_map(|entry| entry["status"].as_str())
        .collect();
    assert_eq!(words, ["replaces", "same", "new", "refused", "refused"]);
    assert_eq!(preview["rules"][3]["code"], "site_rules.invalid_rule");
    assert_eq!(preview["rules"][4]["code"], "site_rules.duplicate_id");

    // Without the question answered, the stored rule stays as it is.
    let (status, imported) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "document": document.clone() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["rules"][0]["status"], "kept");
    assert_eq!(imported["rules"][2]["status"], "stored");
    assert_eq!(
        (imported["stored"].as_u64(), imported["replaced"].as_u64()),
        (Some(1), Some(0))
    );
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(by_id(&listed["rules"], "kept")["name"], "Rule kept");
    assert_eq!(by_id(&listed["rules"], "kept")["origin"]["kind"], "editor");

    // Answered: the file's rule replaces the stored one, its switch with it.
    let (status, imported) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "document": document, "replace": ["kept"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["rules"][0]["status"], "replaced");
    assert_eq!(imported["rules"][2]["status"], "same");
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    let kept = by_id(&listed["rules"], "kept");
    assert_eq!(kept["name"], "Kept, as my colleague wrote it");
    assert_eq!(kept["enabled"], true);
    assert_eq!(kept["origin"]["kind"], "import");
}

/// A file of another layout is refused whole -- the bodies-only export of 1.22 and the signed
/// file among them -- and so is one that is not JSON or carries no document.
#[tokio::test]
async fn a_file_of_another_layout_is_refused_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import/preview",
        json!({ "format_version": 1, "rules": [rule("old", "old.example.org")] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.format_version_unsupported");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "document": { "payload": {}, "signatures": [] } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.malformed");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "format_version": 2, "rules": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["code"], "site_rules.malformed",
        "the file without its envelope"
    );

    let (status, body) =
        post_bytes(&harness.router, "/api/v1/site-rules/import", b"not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.malformed");
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(listed["rules"], json!([]), "a refused file stores nothing");

    // A switch after the import is the person's as ever.
    create(&harness.router, rule("mine", "mine.example.org"), false).await;
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/mine/enabled",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

//! RD-1200-05 and RD-1230-03: every rule records where it came from -- an import, the editor,
//! an MCP tool or the example list -- and the list can be emptied and the examples restored.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_harness};
use serde_json::{Value, json};

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

fn row<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["id"] == id)
        .unwrap_or_else(|| panic!("no rule {id} in {body}"))
}

fn example_ids() -> Vec<String> {
    rd_siterules::examples()
        .into_iter()
        .map(|rule| rule.id)
        .collect()
}

#[tokio::test]
async fn each_path_records_its_origin_and_a_switch_keeps_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "document": {
            "format_version": 2,
            "rules": [{ "enabled": false, "rule": origin_rule("origin-imported") }]
        } }),
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
    let (status, body) = post_json(&harness.router, "/api/v1/site-rules/examples", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(
        row(&body, "origin-imported")["origin"],
        json!({ "kind": "import" })
    );
    assert_eq!(
        row(&body, "origin-written")["origin"],
        json!({ "kind": "editor" })
    );
    assert_eq!(
        row(&body, "debian-cd")["origin"],
        json!({ "kind": "example" })
    );

    // Switching an example on changes nothing about where it came from.
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/debian-cd/enabled",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(row(&body, "debian-cd")["origin"]["kind"], "example");

    // A changed body is the editor's.
    let mut changed = row(&body, "debian-cd")["rule"].clone();
    changed["name"] = json!("Debian, my way");
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/debian-cd",
        json!({ "rule": changed, "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(
        row(&body, "debian-cd")["origin"],
        json!({ "kind": "editor" })
    );
}

/// The first start installs the examples switched off, and only the first: after every rule was
/// deleted, a restart brings none back -- the button does, and leaves a kept example alone.
#[tokio::test]
async fn the_examples_arrive_once_switched_off_and_the_button_restores_them() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let ids = example_ids();
    assert!((3..=6).contains(&ids.len()));

    rd_api::site_rules_service::install_examples_once(&harness.database).await;
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    for id in &ids {
        let rule = row(&body, id);
        assert_eq!(rule["enabled"], false, "{id} arrives switched off");
        assert_eq!(rule["group"], "examples");
        assert!(
            rule["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }
    let catalogue = rd_api::site_rule_catalogue(&harness.database).await;
    assert_eq!(
        catalogue.rules().count(),
        0,
        "no example is consulted unasked"
    );

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/clear",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rd_api::site_rules_service::install_examples_once(&harness.database).await;
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(
        body["rules"],
        json!([]),
        "the second start installs nothing"
    );

    let (status, body) = post_json(&harness.router, "/api/v1/site-rules/examples", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["restored"], ids.len());
    // A kept and changed example stays as it is.
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    let mut changed = row(&listed, &ids[0])["rule"].clone();
    changed["name"] = json!("Mine now");
    let (status, body) = put_json(
        &harness.router,
        &format!("/api/v1/site-rules/{}", ids[0]),
        json!({ "rule": changed, "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = post_json(&harness.router, "/api/v1/site-rules/examples", json!({})).await;
    assert_eq!(body["restored"], 0);
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(row(&listed, &ids[0])["name"], "Mine now");
}

/// Deleting every rule wants its confirmation as a value, takes the self-test results along,
/// keeps the group switches and writes itself into the audit log.
#[tokio::test]
async fn deleting_every_rule_is_confirmed_audited_and_keeps_the_group_switches() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for id in ["one", "two"] {
        let mut rule = origin_rule(id);
        rule["match"]["hosts"] = json!([format!("{id}.example.org")]);
        rule["probe"] = json!(format!("https://{id}.example.org/release/1"));
        let (status, body) = post_json(
            &harness.router,
            "/api/v1/site-rules",
            json!({ "rule": rule, "enabled": true }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rule-groups/board/enabled",
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post_json(&harness.router, "/api/v1/site-rules/clear", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.not_confirmed");
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(listed["rules"].as_array().map(Vec::len), Some(2));

    let audit_before = harness.database.count_audit_records().await.expect("audit");
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/clear",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2);
    assert_eq!(
        harness.database.count_audit_records().await.expect("audit"),
        audit_before + 1
    );
    let (_, listed) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(listed["rules"], json!([]));
    assert_eq!(
        rd_api::site_rule_catalogue(&harness.database)
            .await
            .rules()
            .count(),
        0
    );
    let switches = harness
        .database
        .list_site_rule_switches()
        .await
        .expect("switches");
    assert!(
        switches
            .iter()
            .any(|row| row.key == "board" && !row.enabled),
        "the group switch stays"
    );
}

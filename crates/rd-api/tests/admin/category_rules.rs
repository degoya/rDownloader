//! Changing and deleting a category rule (audit 2026-11-25, TST-7). What a rule matches is
//! `rd-core`'s own suite; the bundle that carries rules is `routing_backup`.

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, put_json, test_router};
use serde_json::{Value, json};

async fn created(router: &axum::Router, uri: &str, body: Value) -> Value {
    let (status, answer) = post_json(router, uri, body).await;
    assert_eq!(status, StatusCode::CREATED, "{uri}: {answer}");
    answer
}

fn rule(name: &str, domain: &str, category: &Value) -> Value {
    json!({
        "name": name,
        "priority": 10,
        "source": null,
        "domain": domain,
        "protocol": null,
        "extension": ".MKV",
        "mime_type": null,
        "name_regex": null,
        "category_id": category,
        "enabled": true,
    })
}

#[tokio::test]
async fn a_rule_is_changed_and_deleted_by_its_id() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let media = directory.path().join("media");
    std::fs::create_dir_all(&media).expect("media");
    let root = created(
        &router,
        "/api/v1/storage-roots",
        json!({ "name": "Main", "path": media.to_string_lossy(), "is_default": true }),
    )
    .await;
    let category = created(
        &router,
        "/api/v1/categories",
        json!({
            "name": "Films",
            "color": "#336699",
            "storage_root_id": root["id"],
            "relative_path": "films",
            "is_default": false,
        }),
    )
    .await;
    let category = &category["id"];
    let stored = created(
        &router,
        "/api/v1/category-rules",
        rule("first", "files.example.com", category),
    )
    .await;
    let uri = format!(
        "/api/v1/category-rules/{}",
        stored["id"].as_str().expect("an id")
    );

    let mut changed = rule("second", "other.example.com", category);
    changed["enabled"] = json!(false);
    let (status, body) = put_json(&router, &uri, changed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], stored["id"], "{body}");
    assert_eq!(body["name"], "second", "{body}");
    assert_eq!(body["domain"], "other.example.com", "{body}");
    assert_eq!(body["extension"], "mkv", "normalised: {body}");
    assert_eq!(body["enabled"], false, "{body}");

    // A rule that could never match is refused, and the stored one stays.
    let bad = rule("third", "https://other.example.com/path", category);
    let (status, body) = put_json(&router, &uri, bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "category_rule.domain_invalid", "{body}");
    let (_, rules) = get_json(&router, "/api/v1/category-rules").await;
    assert_eq!(rules[0]["name"], "second", "{rules}");

    let (status, body) = delete_json(&router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "category_rule.deleted", "{body}");
    let (_, rules) = get_json(&router, "/api/v1/category-rules").await;
    assert_eq!(rules, json!([]));

    for (status, body) in [
        delete_json(&router, &uri).await,
        put_json(&router, &uri, rule("ghost", "ghost.example.com", category)).await,
    ] {
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["code"], "category_rule.not_found", "{body}");
    }
}

//! The order of the LinkFilter rules (audit 2026-11-25, TST-7): the first matching rule decides, so
//! the named ones come first, in the order given, and what is left out follows as it was.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_router};
use serde_json::json;

fn names(rules: &serde_json::Value) -> Vec<&str> {
    rules
        .as_array()
        .expect("a list")
        .iter()
        .map(|rule| rule["name"].as_str().expect("a name"))
        .collect()
}

#[tokio::test]
async fn the_named_rules_come_first_and_the_rest_keeps_its_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let mut ids = Vec::new();
    for name in ["one", "two", "three", "four"] {
        let (status, rule) = post_json(
            &router,
            "/api/v1/link-filters",
            json!({ "name": name, "action": "hide", "extensions": ["nfo"] }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{rule}");
        ids.push(rule["id"].clone());
    }

    let (status, rules) = post_json(
        &router,
        "/api/v1/link-filters/reorder",
        json!({ "ids": [ids[2], ids[0]] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rules}");
    assert_eq!(names(&rules), ["three", "one", "two", "four"]);
    let (_, stored) = get_json(&router, "/api/v1/link-filters").await;
    assert_eq!(names(&stored), ["three", "one", "two", "four"], "stored");
}

//! RD-110-08: the rules a person can see, switch, write, try and carry.
//!
//! What is observable from outside, and therefore what is asserted here: a fresh installation
//! knows no rule at all (RD-130-07; the examples are `serve`'s to install, `site_rule_origin.rs`),
//! a rule can be switched off and stays off across a restart, a group switch covers every rule
//! that carries the group, and a copy of a rule can be edited without touching the original.
//! The exchange file is `site_rule_exchange.rs`.
//!
//! The trial run is not driven against a live site here -- the executor's own suite does that
//! without a network, and this layer adds only the address check and the crawl verdict. What
//! is asserted is the refusal side of it, which is the part this layer owns.

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, put_json, test_harness};
use serde_json::{Value, json};

/// A rule of the person's own: the smallest one `rd_siterules::Rule::validate` accepts.
fn own_rule(id: &str) -> Value {
    json!({
        "id": id,
        "name": "My board",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["example.org"], "paths": ["^/release/"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "href=\"(https?://[^\"]+)\"", "into": "links", "all": true }
        ],
        "package": { "from": "title" },
        "probe": "https://example.org/release/1",
        "checked": "2026-09-21"
    })
}

fn rule_by_id<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["id"] == id)
        .unwrap_or_else(|| panic!("no rule {id} in {body}"))
}

fn group_by_name<'a>(body: &'a Value, group: &str) -> &'a Value {
    body["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .find(|entry| entry["group"] == group)
        .unwrap_or_else(|| panic!("no group {group} in {body}"))
}

#[tokio::test]
async fn a_fresh_installation_knows_no_rule_and_a_rule_switches_off_for_good() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    // Nothing arrives with the binary any more (RD-130-07): no rule listed, none consulted.
    let (status, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rules"], json!([]), "{body}");
    assert_eq!(body["groups"], json!([]), "{body}");
    let catalogue = rd_api::site_rule_catalogue(&harness.database).await;
    assert_eq!(
        catalogue.rules().count(),
        0,
        "a fresh installation consults no rule"
    );

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules",
        json!({ "rule": own_rule("my-board"), "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "site_rules.saved");

    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    let own = rule_by_id(&body, "my-board");
    assert_eq!(own["active"], true);
    assert_eq!(own["steps"], 2);
    assert_eq!(own["rule"]["probe"], "https://example.org/release/1");
    // Nothing has run the self-test in this installation, so the column is empty rather than
    // guessed at (RD-110-09).
    assert_eq!(own["check"], Value::Null);
    assert_eq!(group_by_name(&body, "board")["rules"], 1);

    // Switching it off keeps it in the list and takes it out of the catalogue.
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/my-board/enabled",
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(rule_by_id(&body, "my-board")["enabled"], false);
    assert_eq!(rule_by_id(&body, "my-board")["active"], false);

    // And it is still off after a restart: a second service over the same database.
    let restarted = test_harness(directory.path()).await;
    let (_, body) = get_json(&restarted.router, "/api/v1/site-rules").await;
    assert_eq!(rule_by_id(&body, "my-board")["enabled"], false);
    let catalogue = rd_api::site_rule_catalogue(&restarted.database).await;
    assert!(
        catalogue.get("my-board").is_none(),
        "a switched-off rule must not be consulted after a restart"
    );
}

#[tokio::test]
async fn a_group_switch_covers_every_rule_that_carries_the_group() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let mut paste = own_rule("my-paste");
    paste["group"] = json!("paste");
    for rule in [own_rule("my-board"), own_rule("other-board"), paste] {
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
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(group_by_name(&body, "board")["enabled"], false);
    assert_eq!(group_by_name(&body, "board")["rules"], 2);
    // The rules keep their own switch; only "active" follows the group.
    assert_eq!(rule_by_id(&body, "my-board")["enabled"], true);
    assert_eq!(rule_by_id(&body, "my-board")["active"], false);
    assert_eq!(rule_by_id(&body, "other-board")["active"], false);
    assert_eq!(rule_by_id(&body, "my-paste")["active"], true);
    let catalogue = rd_api::site_rule_catalogue(&harness.database).await;
    assert!(catalogue.get("my-board").is_none());
    assert!(catalogue.get("other-board").is_none());
    assert!(catalogue.get("my-paste").is_some());

    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rule-groups/nonesuch/enabled",
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "site_rules.group_not_found");

    // An id is taken once: a second create under it is refused, not written over the first.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules",
        json!({ "rule": own_rule("my-board"), "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "site_rules.duplicate_id");
    let (status, body) = delete_json(&harness.router, "/api/v1/site-rules/other-board").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "site_rules.deleted");
}

/// RD-130-07: a copy is a rule of its own. The settings page duplicates by creating the copy
/// under a new id; editing the copy afterwards leaves the original exactly as it was.
#[tokio::test]
async fn a_copy_of_a_rule_is_edited_without_touching_the_original() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules",
        json!({ "rule": own_rule("my-board"), "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut copy = own_rule("my-board-copy");
    copy["name"] = json!("My board (copy)");
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules",
        json!({ "rule": copy, "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut edited = copy.clone();
    edited["match"]["hosts"] = json!(["mirror.example.org"]);
    edited["probe"] = json!("https://mirror.example.org/release/1");
    edited["version"] = json!(2);
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/my-board-copy",
        json!({ "rule": edited, "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    let original = rule_by_id(&body, "my-board");
    assert_eq!(original["hosts"], json!(["example.org"]));
    assert_eq!(original["version"], 1);
    assert_eq!(original["enabled"], true);
    assert_eq!(original["rule"]["probe"], "https://example.org/release/1");
    let copy = rule_by_id(&body, "my-board-copy");
    assert_eq!(copy["name"], "My board (copy)");
    assert_eq!(copy["hosts"], json!(["mirror.example.org"]));
    assert_eq!(copy["version"], 2);
    assert_eq!(copy["enabled"], false);
}

#[tokio::test]
async fn the_trial_run_refuses_what_it_cannot_try() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/test",
        json!({ "rule": own_rule("my-board"), "address": "not an address" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.invalid_address");

    let mut broken = own_rule("my-board");
    broken["match"]["hosts"] = json!([]);
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/test",
        json!({ "rule": broken, "address": "https://example.org/release/1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.invalid_rule");

    // A rule that does not claim the address answers with the executor's own code rather
    // than with a failure of the request: the run happened, and it said nothing was its.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/test",
        json!({ "rule": own_rule("my-board"), "address": "https://elsewhere.test/release/1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["error"], "site_rules.not_claimed");
    assert_eq!(body["links"].as_array().expect("links").len(), 0);
}

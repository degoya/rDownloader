//! RD-110-08: the rules a person can see, switch, write, try and carry.
//!
//! What is observable from outside, and therefore what is asserted here: a fresh installation
//! knows no rule at all (RD-130-07), a rule can be switched off and stays off across a restart,
//! a group switch covers every rule that carries the group, an imported rule arrives switched
//! off no matter what the file asked for, the signed release file imports as the rules that
//! used to ship, and a copy of a rule can be edited without touching the original.
//!
//! The trial run is not driven against a live site here -- the executor's own suite does that
//! without a network, and this layer adds only the address check and the crawl verdict. What
//! is asserted is the refusal side of it, which is the part this layer owns.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{delete_json, get_json, post_json, put_json, send, test_harness};
use serde_json::{Value, json};

/// The signed rule file every release carries as an artifact (RD-130-07).
const RELEASE_FILE: &[u8] = include_bytes!("../../rd-siterules/resources/site-rules.json");

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

/// Posts a file to the import exactly as it lies on disk. `post_json` would parse and
/// re-serialise it, and the signature covers the bytes as they were signed.
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

#[tokio::test]
async fn an_imported_rule_is_stored_switched_off_whatever_the_file_says() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    // A file with three rules: one good, one whose body does not validate, and a repeat of
    // the good one.
    let mut broken = own_rule("broken");
    broken["steps"] = json!([]);
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({
            "format_version": 1,
            "rules": [own_rule("from-a-friend"), broken, own_rule("from-a-friend")]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stored"], 1);
    assert_eq!(body["signed"], false);
    let outcomes = body["rules"].as_array().expect("rules");
    assert_eq!(outcomes[0]["status"], "stored");
    assert_eq!(outcomes[1]["code"], "site_rules.invalid_rule");
    assert_eq!(outcomes[2]["code"], "site_rules.duplicate_id");

    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    let imported = rule_by_id(&body, "from-a-friend");
    assert_eq!(
        imported["enabled"], false,
        "an unsigned rule is never active before somebody says so"
    );
    let catalogue = rd_api::site_rule_catalogue(&harness.database).await;
    assert!(catalogue.get("from-a-friend").is_none());

    // The confirmation is a request of its own, per rule.
    let (status, body) = put_json(
        &harness.router,
        "/api/v1/site-rules/from-a-friend/enabled",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let catalogue = rd_api::site_rule_catalogue(&harness.database).await;
    assert!(catalogue.get("from-a-friend").is_some());

    // The export carries the rules this installation holds.
    let (status, exported) = get_json(&harness.router, "/api/v1/site-rules/export").await;
    assert_eq!(status, StatusCode::OK, "{exported}");
    assert_eq!(exported["format_version"], 1);
    let rules = exported["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"], "from-a-friend");

    // A file in a format this build does not read is refused whole.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/site-rules/import",
        json!({ "format_version": 2, "rules": [own_rule("later")] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.format_version_unsupported");

    // And so is one that is not JSON at all.
    let (status, body) = import_bytes(&harness.router, b"not json".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.malformed");
}

/// RD-130-07: the file that replaces the compiled-in pack delivers the same rules, as the
/// person's own and switched off, and only while its signature holds.
#[tokio::test]
async fn the_signed_release_file_imports_the_rules_that_used_to_ship() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let pack = rd_siterules::verify(RELEASE_FILE, None, chrono::Utc::now())
        .expect("the release file verifies");
    let ids: Vec<&str> = pack.rules.iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "scnlog",
            "downmagaz",
            "paste-generic",
            "getcomics",
            "scene-rls",
            "avaxhome",
            "cgpersia",
            "vipergirls",
        ],
        "the eight rules 1.2 shipped"
    );

    // A file whose payload was altered after signing is refused whole, and stores nothing.
    let tampered = String::from_utf8(RELEASE_FILE.to_vec())
        .expect("utf-8")
        .replacen("scnlog.me", "scnlog.test", 1)
        .into_bytes();
    let (status, body) = import_bytes(&harness.router, tampered).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.bad_signature");
    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    assert_eq!(
        body["rules"],
        json!([]),
        "a refused file leaves nothing behind"
    );

    let (status, body) = import_bytes(&harness.router, RELEASE_FILE.to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["signed"], true);
    assert_eq!(body["stored"], pack.rules.len());

    let (_, body) = get_json(&harness.router, "/api/v1/site-rules").await;
    for rule in &pack.rules {
        let row = rule_by_id(&body, &rule.id);
        assert_eq!(
            row["rule"],
            serde_json::to_value(rule).expect("rule"),
            "{} arrives exactly as it was signed",
            rule.id
        );
        assert_eq!(row["enabled"], false, "{} arrives switched off", rule.id);
    }
    // The groups are the five the settings page has a name for.
    let mut groups: Vec<&str> = body["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .filter_map(|group| group["group"].as_str())
        .collect();
    groups.sort_unstable();
    assert_eq!(groups, ["adult", "board", "ebooks", "graphics", "paste"]);
    let catalogue = rd_api::site_rule_catalogue(&harness.database).await;
    assert_eq!(
        catalogue.rules().count(),
        0,
        "nothing imported is consulted yet"
    );

    // A second import meets the rules the first one stored and writes over none of them.
    let (status, body) = import_bytes(&harness.router, RELEASE_FILE.to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stored"], 0);
    assert!(
        body["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .all(|entry| entry["code"] == "site_rules.duplicate_id"),
        "{body}"
    );
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

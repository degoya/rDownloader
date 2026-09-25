//! User-written site rules survive a restart (RD-110-04).
//!
//! The database stores a rule as the JSON the system boundary validated and hands it back
//! unchanged; what it does know is the rule's identity, its group and whether it is switched
//! on, so a list can be drawn without parsing every body.

use rd_core::EventKind;
use rd_db::{Database, NewUserSiteRule};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("rules.sqlite"))
        .await
        .expect("database")
}

fn rule(id: &str) -> NewUserSiteRule {
    NewUserSiteRule {
        id: id.to_owned(),
        name: format!("{id} board"),
        group: "board".to_owned(),
        enabled: true,
        rule: serde_json::json!({ "id": id, "steps": [{ "kind": "fetch" }] }),
    }
}

fn drain(
    events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>,
) -> Vec<serde_json::Value> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::SiteRuleChanged {
            seen.push(event.payload);
        }
    }
    seen
}

#[tokio::test]
async fn a_user_rule_is_still_there_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(&directory).await;
        let stored = database
            .upsert_site_rule(rule("my-board"))
            .await
            .expect("upsert");
        assert_eq!(stored.id, "my-board");
        assert_eq!(stored.created_at, stored.updated_at);
    }
    let database = database(&directory).await;
    let rules = database.list_site_rules().await.expect("list");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "my-board");
    assert_eq!(rules[0].group, "board");
    assert!(rules[0].enabled);
    assert_eq!(rules[0].rule["steps"][0]["kind"], "fetch");
}

#[tokio::test]
async fn an_upsert_replaces_the_body_and_keeps_the_creation_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let first = database
        .upsert_site_rule(rule("my-board"))
        .await
        .expect("upsert");
    let mut changed = rule("my-board");
    changed.enabled = false;
    changed.rule["steps"][0]["into"] = serde_json::json!("html");
    let second = database.upsert_site_rule(changed).await.expect("upsert");
    assert_eq!(second.created_at, first.created_at);
    assert!(!second.enabled);
    let rules = database.list_site_rules().await.expect("list");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule["steps"][0]["into"], "html");
}

#[tokio::test]
async fn rules_are_listed_by_id_and_a_delete_says_whether_it_removed_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .upsert_site_rule(rule("zeta"))
        .await
        .expect("upsert");
    database
        .upsert_site_rule(rule("alpha"))
        .await
        .expect("upsert");
    let ids: Vec<String> = database
        .list_site_rules()
        .await
        .expect("list")
        .into_iter()
        .map(|rule| rule.id)
        .collect();
    assert_eq!(ids, ["alpha", "zeta"]);
    assert!(database.delete_site_rule("zeta").await.expect("delete"));
    assert!(!database.delete_site_rule("zeta").await.expect("delete"));
    assert_eq!(database.list_site_rules().await.expect("list").len(), 1);
}

/// Every write announces exactly one change naming the rule, and a delete that removed
/// nothing announces nothing.
#[tokio::test]
async fn every_write_announces_exactly_one_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();
    database
        .upsert_site_rule(rule("mine"))
        .await
        .expect("upsert");
    let payloads = drain(&mut events);
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0]["rule_id"], "mine");
    assert!(
        payloads[0].get("rule").is_none(),
        "the body stays in the table"
    );
    database.delete_site_rule("mine").await.expect("delete");
    assert_eq!(drain(&mut events).len(), 1);
    database.delete_site_rule("mine").await.expect("delete");
    assert!(drain(&mut events).is_empty());
}

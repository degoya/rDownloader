//! User-written site rules survive a restart (RD-110-04).
//!
//! The database stores a rule as the JSON the system boundary validated and hands it back
//! unchanged; what it does know is the rule's identity, its group and whether it is switched
//! on, so a list can be drawn without parsing every body.

use rd_core::EventKind;
use rd_db::{Database, NewUserSiteRule, SiteRuleOrigin, SiteRuleOriginKind};
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
        origin: SiteRuleOrigin::unsigned(SiteRuleOriginKind::Editor),
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

/// The origin is stored with the body and replaced with it (RD-1200-05): a signed rule keeps
/// its signer and sequence across a restart, and an edit that writes another origin drops them.
#[tokio::test]
async fn a_rule_keeps_where_it_came_from_until_its_body_is_replaced() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(&directory).await;
        let mut signed = rule("from-the-file");
        signed.origin = SiteRuleOrigin::signed("rdownloader-siterules-v1", 9);
        database.upsert_site_rule(signed).await.expect("upsert");
    }
    let database = database(&directory).await;
    let stored = database.list_site_rules().await.expect("list");
    assert_eq!(
        stored[0].origin,
        SiteRuleOrigin::signed("rdownloader-siterules-v1", 9)
    );
    let mut edited = rule("from-the-file");
    edited.origin = SiteRuleOrigin::unsigned(SiteRuleOriginKind::Mcp);
    let edited = database.upsert_site_rule(edited).await.expect("upsert");
    assert_eq!(edited.origin.kind, SiteRuleOriginKind::Mcp);
    assert_eq!(edited.origin.signer, None);
    assert_eq!(edited.origin.sequence, None);
}

/// The mark per signer only rises, and each call answers the mark before it (RD-1200-05).
#[tokio::test]
async fn the_highest_sequence_per_signer_only_rises() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let signer = "rdownloader-siterules-v1";
    assert_eq!(
        database
            .record_site_rule_pack(signer, 9)
            .await
            .expect("first"),
        None
    );
    assert_eq!(
        database
            .record_site_rule_pack(signer, 9)
            .await
            .expect("same"),
        Some(9)
    );
    assert_eq!(
        database
            .record_site_rule_pack(signer, 7)
            .await
            .expect("older"),
        Some(9)
    );
    // The older file did not lower the mark.
    assert_eq!(
        database
            .record_site_rule_pack(signer, 9)
            .await
            .expect("again"),
        Some(9)
    );
    assert_eq!(
        database
            .record_site_rule_pack(signer, 10)
            .await
            .expect("newer"),
        Some(9)
    );
    assert_eq!(
        database
            .record_site_rule_pack(signer, 10)
            .await
            .expect("now"),
        Some(10)
    );
    // Another signer has a mark of its own.
    assert_eq!(
        database
            .record_site_rule_pack("other", 1)
            .await
            .expect("other"),
        None
    );
}

//! User-written site rules survive a restart (RD-110-04).
//!
//! The database stores a rule as the JSON the system boundary validated and hands it back
//! unchanged; what it does know is the rule's identity, its group and whether it is switched
//! on, so a list can be drawn without parsing every body.

use rd_core::EventKind;
use rd_db::{Database, NewSiteRuleCheck, NewUserSiteRule, SiteRuleOriginKind};
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
        origin: SiteRuleOriginKind::Editor,
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

/// The origin is stored with the body and replaced with it (RD-1200-05): an example keeps its
/// origin across a restart, and an edit that writes another origin replaces it.
#[tokio::test]
async fn a_rule_keeps_where_it_came_from_until_its_body_is_replaced() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(&directory).await;
        let mut example = rule("debian-cd");
        example.origin = SiteRuleOriginKind::Example;
        database.upsert_site_rule(example).await.expect("upsert");
    }
    let database = database(&directory).await;
    let stored = database.list_site_rules().await.expect("list");
    assert_eq!(stored[0].origin, SiteRuleOriginKind::Example);
    let mut edited = rule("debian-cd");
    edited.origin = SiteRuleOriginKind::Mcp;
    let edited = database.upsert_site_rule(edited).await.expect("upsert");
    assert_eq!(edited.origin, SiteRuleOriginKind::Mcp);
}

/// Every word the origin column holds reads back as what wrote it; `signed`, which 1.20 to 1.22
/// wrote, reads as unknown since the signature is gone (RD-1230-03).
#[test]
fn every_origin_word_reads_back_and_a_retired_one_is_unknown() {
    for kind in [
        SiteRuleOriginKind::Import,
        SiteRuleOriginKind::Editor,
        SiteRuleOriginKind::Mcp,
        SiteRuleOriginKind::Example,
        SiteRuleOriginKind::Unknown,
    ] {
        assert_eq!(SiteRuleOriginKind::parse(kind.as_str()), kind);
    }
    assert_eq!(
        SiteRuleOriginKind::parse("signed"),
        SiteRuleOriginKind::Unknown
    );
}

/// Deleting every rule takes the self-test results with it, keeps the group switches and
/// announces one change -- and none when there was nothing to delete (RD-1230-03).
#[tokio::test]
async fn deleting_every_rule_takes_the_checks_and_keeps_the_group_switches() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    for id in ["alpha", "beta", "gamma"] {
        database.upsert_site_rule(rule(id)).await.expect("upsert");
    }
    database
        .record_site_rule_checks(vec![NewSiteRuleCheck {
            rule_id: "alpha".to_owned(),
            verdict: "dead".to_owned(),
            code: Some("site_rules.page_dead".to_owned()),
            links: 0,
            pages: 1,
        }])
        .await
        .expect("record");
    database
        .set_site_rule_switch(rd_db::SCOPE_GROUP, "board", false)
        .await
        .expect("switch");
    let mut events = database.subscribe();

    assert_eq!(database.delete_all_site_rules().await.expect("delete"), 3);
    assert!(database.list_site_rules().await.expect("list").is_empty());
    assert!(
        database
            .list_site_rule_checks()
            .await
            .expect("checks")
            .is_empty()
    );
    let switches = database.list_site_rule_switches().await.expect("switches");
    assert!(
        switches
            .iter()
            .any(|row| row.key == "board" && !row.enabled),
        "the group stays switched off"
    );
    let payloads = drain(&mut events);
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0]["removed"], 3);

    assert_eq!(database.delete_all_site_rules().await.expect("again"), 0);
    assert!(drain(&mut events).is_empty());
}

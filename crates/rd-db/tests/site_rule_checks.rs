//! What the rule self-test found survives a restart (RD-110-09).
//!
//! The result is keyed by the rule's own identifier and lives in its own table, because the
//! rules the project ships have no row in `site_rules` at all. The list in RD-110-08 and the
//! skip list the crawler selection uses both read it back from here.

use rd_core::EventKind;
use rd_db::{Database, NewSiteRuleCheck};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("checks.sqlite"))
        .await
        .expect("database")
}

fn check(rule_id: &str, verdict: &str, code: Option<&str>) -> NewSiteRuleCheck {
    NewSiteRuleCheck {
        rule_id: rule_id.to_owned(),
        verdict: verdict.to_owned(),
        code: code.map(str::to_owned),
        links: if verdict == "ok" { 7 } else { 0 },
        pages: 1,
    }
}

#[tokio::test]
async fn a_result_is_still_there_after_a_restart_and_names_a_rule_with_no_row_of_its_own() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(&directory).await;
        database
            .record_site_rule_checks(vec![
                check("scnlog", "ok", None),
                check("gpaste", "dead", Some("site_rules.page_dead")),
            ])
            .await
            .expect("record");
    }
    let database = database(&directory).await;
    let stored = database.list_site_rule_checks().await.expect("list");
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].rule_id, "gpaste");
    assert_eq!(stored[0].verdict, "dead");
    assert_eq!(stored[0].code.as_deref(), Some("site_rules.page_dead"));
    assert_eq!(stored[1].rule_id, "scnlog");
    assert_eq!(stored[1].verdict, "ok");
    assert_eq!(stored[1].code, None);
    assert_eq!(stored[1].links, 7);
    assert!(!stored[1].checked_at.is_empty());
    // No user rule was ever written; a result exists for a shipped rule all the same.
    assert!(database.list_site_rules().await.expect("list").is_empty());
}

#[tokio::test]
async fn a_later_run_replaces_what_an_earlier_one_said() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .record_site_rule_checks(vec![check("scnlog", "ok", None)])
        .await
        .expect("record");
    database
        .record_site_rule_checks(vec![check(
            "scnlog",
            "structural",
            Some("site_rules.structure"),
        )])
        .await
        .expect("record");
    let stored = database.list_site_rule_checks().await.expect("list");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].verdict, "structural");
    assert_eq!(stored[0].code.as_deref(), Some("site_rules.structure"));
    assert_eq!(stored[0].links, 0);
}

/// One run, one announcement: a list that redrew itself per rule would flicker through a
/// pack of twenty.
#[tokio::test]
async fn one_run_announces_exactly_one_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();
    database
        .record_site_rule_checks(vec![
            check("one", "ok", None),
            check("two", "blocked", Some("site_rules.blocked")),
            check("three", "dead", Some("site_rules.page_dead")),
        ])
        .await
        .expect("record");
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::SiteRuleChanged {
            seen.push(event.payload);
        }
    }
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["resource"], "site_rule_check");
    assert_eq!(seen[0]["rules"], 3);
}

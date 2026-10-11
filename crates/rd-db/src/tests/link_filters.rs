//! LinkFilter rules (RD-1240-09): at intake, applied again to the list, and what the queue takes.

use rd_core::{IngressSource, LinkFilterAction, LinkFilterNameSyntax, LinkFilterRule};

use super::{routing_category, routing_root};
use crate::{Database, NewCollectorBatch, NewLinkFilterRule};

async fn open(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("filters.sqlite"))
        .await
        .expect("database")
}

fn hide(name: &str, pattern: &str) -> NewLinkFilterRule {
    NewLinkFilterRule {
        name: name.to_owned(),
        enabled: true,
        name_pattern: Some(pattern.to_owned()),
        name_syntax: LinkFilterNameSyntax::Glob,
        size_min: None,
        size_max: None,
        extensions: Vec::new(),
        hoster: None,
        source: None,
        action: LinkFilterAction::Hide,
        package_name: None,
        category_id: None,
    }
}

async fn create(database: &Database, rule: NewLinkFilterRule) -> LinkFilterRule {
    database
        .create_link_filter_rule(rule)
        .await
        .expect("create a rule")
}

/// One release with its info file, pasted.
async fn release(database: &Database) -> Vec<rd_core::LinkCandidate> {
    let (_, _, candidates) = database
        .add_collector_batch(NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Clipboard,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec![
                "https://files.example/Show.S01E01.mkv"
                    .parse()
                    .expect("url"),
                "https://files.example/Show.S01E01.nfo"
                    .parse()
                    .expect("url"),
            ],
            providers: vec![None, None],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    candidates
}

fn by_name<'a>(candidates: &'a [rd_core::LinkCandidate], name: &str) -> &'a rd_core::LinkCandidate {
    candidates
        .iter()
        .find(|candidate| candidate.file_name.as_deref() == Some(name))
        .expect("the link")
}

#[tokio::test]
async fn a_hiding_rule_keeps_the_link_and_a_whole_package_enqueue_leaves_it_behind() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let rule = create(&database, hide("Info files", "*.NFO")).await;
    let candidates = release(&database).await;
    let nfo = by_name(&candidates, "Show.S01E01.nfo");
    let mkv = by_name(&candidates, "Show.S01E01.mkv");
    assert_eq!(nfo.hidden_by_filter, Some(rule.id));
    assert_eq!(mkv.hidden_by_filter, None);
    // Hidden is not deleted: the link is listed, in its package.
    assert_eq!(database.list_candidates().await.expect("list").len(), 2);

    let package = mkv.package_id.expect("package");
    let claimed = database
        .claim_package_for_enqueue(package, None)
        .await
        .expect("claim");
    assert_eq!(
        claimed
            .iter()
            .map(|(candidate, _)| candidate.id)
            .collect::<Vec<_>>(),
        vec![mkv.id]
    );
    database
        .finish_package_enqueue(package, true, Vec::new())
        .await
        .expect("finish");
    let left = database.list_candidates().await.expect("list");
    let kept = left
        .iter()
        .find(|candidate| candidate.id == nfo.id)
        .expect("the hidden link stays in the LinkGrabber");
    assert_eq!(kept.package_id, Some(package));
    assert_eq!(kept.state, rd_core::LinkCandidateState::Online);
}

#[tokio::test]
async fn a_hidden_link_named_by_the_enqueue_is_claimed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    create(&database, hide("Info files", "*.nfo")).await;
    let candidates = release(&database).await;
    let nfo = by_name(&candidates, "Show.S01E01.nfo");
    let claimed = database
        .claim_package_for_enqueue(nfo.package_id.expect("package"), Some(vec![nfo.id]))
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].0.id, nfo.id);
}

#[tokio::test]
async fn an_accept_above_a_hide_keeps_the_link_and_order_is_the_position() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let hiding = create(&database, hide("Everything", "*")).await;
    let accept = create(
        &database,
        NewLinkFilterRule {
            action: LinkFilterAction::Accept,
            ..hide("Videos", "*.mkv")
        },
    )
    .await;
    // Created second, the accept is asked second: the hide takes both links.
    let first = release(&database).await;
    assert!(
        first
            .iter()
            .all(|candidate| candidate.hidden_by_filter == Some(hiding.id))
    );
    database
        .reorder_link_filter_rules(vec![accept.id])
        .await
        .expect("reorder");
    let rules = database.list_link_filter_rules().await.expect("rules");
    assert_eq!(
        rules
            .iter()
            .map(|rule| (rule.id, rule.position))
            .collect::<Vec<_>>(),
        vec![(accept.id, 1), (hiding.id, 2)]
    );
    // Applied again, the accept now comes first and shows the video; the info file stays hidden.
    let outcome = database.apply_link_filters().await.expect("apply");
    assert_eq!(outcome.shown, 1);
    assert_eq!(outcome.hidden, 0);
    let list = database.list_candidates().await.expect("list");
    assert_eq!(by_name(&list, "Show.S01E01.mkv").hidden_by_filter, None);
    assert_eq!(
        by_name(&list, "Show.S01E01.nfo").hidden_by_filter,
        Some(hiding.id)
    );
}

#[tokio::test]
async fn applying_hides_what_arrived_before_the_rule_and_deleting_it_shows_it_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    release(&database).await;
    let rule = create(&database, hide("Info files", "*.nfo")).await;
    // A rule changes nothing that is already listed until it is applied.
    let before = database.list_candidates().await.expect("list");
    assert!(
        before
            .iter()
            .all(|candidate| candidate.hidden_by_filter.is_none())
    );
    let outcome = database.apply_link_filters().await.expect("apply");
    assert_eq!(outcome.hidden, 1);
    let list = database.list_candidates().await.expect("list");
    assert_eq!(
        by_name(&list, "Show.S01E01.nfo").hidden_by_filter,
        Some(rule.id)
    );

    database
        .delete_link_filter_rule(rule.id)
        .await
        .expect("delete");
    let list = database.list_candidates().await.expect("list");
    assert!(
        list.iter()
            .all(|candidate| candidate.hidden_by_filter.is_none())
    );
}

#[tokio::test]
async fn showing_a_hidden_link_lasts_until_the_rules_are_applied_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    create(&database, hide("Info files", "*.nfo")).await;
    let candidates = release(&database).await;
    let nfo = by_name(&candidates, "Show.S01E01.nfo").id;
    let mkv = by_name(&candidates, "Show.S01E01.mkv").id;
    assert_eq!(
        database
            .show_filtered_candidates(vec![nfo, mkv])
            .await
            .expect("show"),
        1
    );
    let list = database.list_candidates().await.expect("list");
    assert!(
        list.iter()
            .all(|candidate| candidate.hidden_by_filter.is_none())
    );
    assert_eq!(
        database.apply_link_filters().await.expect("apply").hidden,
        1
    );
}

#[tokio::test]
async fn a_route_rule_files_the_link_at_intake_and_when_applied() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let root = routing_root(&database, directory.path()).await;
    let extras = routing_category(&database, root, "Extras", false).await;
    // Listed before the rule: only the re-application can file it.
    release(&database).await;
    create(
        &database,
        NewLinkFilterRule {
            action: LinkFilterAction::Route,
            package_name: Some("Info files".to_owned()),
            category_id: Some(extras.id),
            ..hide("Info to extras", "*.nfo")
        },
    )
    .await;
    let outcome = database.apply_link_filters().await.expect("apply");
    assert_eq!(outcome.routed, 1);
    let packages = database.list_collector_packages().await.expect("packages");
    let info = packages
        .iter()
        .find(|package| package.name == "Info files")
        .expect("the rule's package");
    assert_eq!(info.category_id, Some(extras.id));
    let list = database.list_candidates().await.expect("list");
    let nfo = by_name(&list, "Show.S01E01.nfo");
    assert_eq!(nfo.package_id, Some(info.id));
    assert_eq!(nfo.category_id, Some(extras.id));
    assert_eq!(nfo.hidden_by_filter, None);
    // The video stays where it was.
    assert_ne!(by_name(&list, "Show.S01E01.mkv").package_id, Some(info.id));

    // At intake the rule decides before the grouping: a new paste lands there directly.
    let fresh = release(&database).await;
    let filed = fresh
        .iter()
        .find(|candidate| candidate.file_name.as_deref() == Some("Show.S01E01.nfo"))
        .expect("the info file");
    let package = database
        .get_collector_package(filed.package_id.expect("package"))
        .await
        .expect("read")
        .expect("package");
    assert_eq!(package.name, "Info files");
    assert_eq!(package.category_id, Some(extras.id));
}

#[tokio::test]
async fn a_rule_changes_no_queued_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let candidates = release(&database).await;
    let package = candidates[0].package_id.expect("package");
    let claimed = database
        .claim_package_for_enqueue(package, None)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 2);
    database
        .finish_package_enqueue(package, true, Vec::new())
        .await
        .expect("finish");
    create(&database, hide("Everything", "*")).await;
    let outcome = database.apply_link_filters().await.expect("apply");
    assert_eq!(outcome, crate::LinkFilterOutcome::default());
}

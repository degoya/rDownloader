//! LinkFilter "without resolver" and "delete" (RD-1310-01): the rules decide a link once more
//! when its online check ends, a deleting rule removes it, at intake as after the check, and a
//! dry run of "Apply" counts without writing.

use rd_core::{IngressSource, LinkCandidate, LinkFilterAction, LinkFilterRule, LinkStatus};

use super::link_filters::{create, hide};
use crate::{Database, NewCollectorBatch, NewLinkFilterRule};

const GITHUB: &str = "https://github.com/owner/repo";
const GITHUB_TOO: &str = "https://github.com/owner/other";
const DIRECT_FILE: &str = "https://files.example/install";
const HOSTER: &str = "https://hoster.example/file/abc";
const HOSTER_PAGE: &str = "https://hoster.example/folder/abc";

/// A rule on the check verdict for links from the clipboard.
fn without_resolver(action: LinkFilterAction) -> NewLinkFilterRule {
    NewLinkFilterRule {
        name_pattern: None,
        source: Some(IngressSource::Clipboard),
        without_resolver: true,
        action,
        ..hide("Pages without resolver", "*")
    }
}

/// Links copied to the clipboard, each with its package hint, waiting for their check.
async fn clipboard(database: &Database, links: &[(&str, &str)]) -> Vec<LinkCandidate> {
    let (_, _, candidates) = database
        .add_collector_batch(NewCollectorBatch {
            package_hints: links
                .iter()
                .map(|(_, package)| Some((*package).to_owned()))
                .collect(),
            mirror_hints: Vec::new(),
            source: IngressSource::Clipboard,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: links
                .iter()
                .map(|(url, _)| url.parse().expect("url"))
                .collect(),
            providers: vec![None; links.len()],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    candidates
}

fn by_url<'a>(candidates: &'a [LinkCandidate], url: &str) -> Option<&'a LinkCandidate> {
    candidates
        .iter()
        .find(|candidate| candidate.url.as_str() == url)
}

/// Records the check of `url` the way the link check service does: a page answer carries the
/// code of `unresolvable_message`, a file answer none.
async fn check(database: &Database, candidates: &[LinkCandidate], url: &str, code: Option<&str>) {
    let candidate = by_url(candidates, url).expect("the link");
    let status = if code.is_some() {
        LinkStatus::Unresolvable
    } else {
        LinkStatus::Online
    };
    database
        .record_candidate_check(
            candidate.id,
            Some(rd_core::LinkCheckResult {
                url: candidate.url.clone(),
                status,
                file_name: None,
                size: None,
                media: None,
            }),
            code.map(|code| rd_core::CandidateMessage::coded(code, "page")),
            false,
            None,
        )
        .await
        .expect("record the check");
}

/// The four checks of the tester's case: two GitHub pages without a resolver, a direct file
/// without one, a file of an installed hoster and that hoster's page.
async fn check_all(database: &Database, candidates: &[LinkCandidate]) {
    for url in [GITHUB, GITHUB_TOO] {
        check(
            database,
            candidates,
            url,
            Some(rd_core::CODE_CHECK_NO_RESOLVER),
        )
        .await;
    }
    check(database, candidates, DIRECT_FILE, None).await;
    check(database, candidates, HOSTER, None).await;
    check(
        database,
        candidates,
        HOSTER_PAGE,
        Some("collector.check_not_a_file"),
    )
    .await;
}

const TESTER_LINKS: [(&str, &str); 5] = [
    (GITHUB, "GitHub"),
    (GITHUB_TOO, "Mixed"),
    (DIRECT_FILE, "Mixed"),
    (HOSTER, "Hoster"),
    (HOSTER_PAGE, "Hoster"),
];

async fn package_names(database: &Database) -> Vec<String> {
    let mut names: Vec<String> = database
        .list_collector_packages()
        .await
        .expect("packages")
        .into_iter()
        .map(|package| package.name)
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn without_resolver_hides_only_the_pages_the_check_found_without_a_resolver() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    let rule: LinkFilterRule = create(&database, without_resolver(LinkFilterAction::Hide)).await;
    let candidates = clipboard(&database, &TESTER_LINKS).await;
    // Before the check the condition does not hold, at intake or applied again.
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.hidden_by_filter.is_none())
    );
    assert_eq!(
        database.preview_link_filters().await.expect("preview"),
        crate::LinkFilterOutcome::default()
    );

    check_all(&database, &candidates).await;
    let list = database.list_candidates().await.expect("list");
    assert_eq!(list.len(), 5, "a hidden link is kept");
    for url in [GITHUB, GITHUB_TOO] {
        assert_eq!(
            by_url(&list, url).expect("listed").hidden_by_filter,
            Some(rule.id),
            "{url}"
        );
    }
    for url in [DIRECT_FILE, HOSTER, HOSTER_PAGE] {
        assert_eq!(
            by_url(&list, url).expect("listed").hidden_by_filter,
            None,
            "{url}"
        );
    }
}

#[tokio::test]
async fn deleting_after_the_check_takes_the_links_and_only_the_package_they_empty() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    create(&database, without_resolver(LinkFilterAction::Delete)).await;
    let candidates = clipboard(&database, &TESTER_LINKS).await;
    assert_eq!(candidates.len(), 5, "nothing is deleted before the check");

    check_all(&database, &candidates).await;
    let list = database.list_candidates().await.expect("list");
    assert!(by_url(&list, GITHUB).is_none());
    assert!(by_url(&list, GITHUB_TOO).is_none());
    for url in [DIRECT_FILE, HOSTER, HOSTER_PAGE] {
        assert!(by_url(&list, url).is_some(), "{url} stays");
    }
    // "GitHub" lost its only link; "Mixed" keeps the direct file.
    assert_eq!(package_names(&database).await, vec!["Hoster", "Mixed"]);
}

#[tokio::test]
async fn applying_a_deleting_rule_counts_the_deleted_links_and_a_dry_run_writes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    // Checked before the rule exists: only "Apply" can delete them.
    let candidates = clipboard(&database, &TESTER_LINKS).await;
    check_all(&database, &candidates).await;
    create(&database, without_resolver(LinkFilterAction::Delete)).await;

    let preview = database.preview_link_filters().await.expect("preview");
    assert_eq!(preview.deleted, 2);
    assert_eq!(database.list_candidates().await.expect("list").len(), 5);
    assert_eq!(
        package_names(&database).await,
        vec!["GitHub", "Hoster", "Mixed"]
    );

    let outcome = database.apply_link_filters().await.expect("apply");
    assert_eq!(outcome.deleted, 2);
    assert_eq!(outcome.hidden, 0);
    assert_eq!(database.list_candidates().await.expect("list").len(), 3);
    assert_eq!(package_names(&database).await, vec!["Hoster", "Mixed"]);
}

#[tokio::test]
async fn a_deleting_rule_on_an_intake_condition_deletes_at_intake() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    create(
        &database,
        NewLinkFilterRule {
            name_pattern: None,
            hoster: Some("github.com".to_owned()),
            action: LinkFilterAction::Delete,
            ..hide("No GitHub", "*")
        },
    )
    .await;
    let candidates = clipboard(&database, &[(GITHUB, "GitHub"), (DIRECT_FILE, "Mixed")]).await;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].url.as_str(), DIRECT_FILE);
    assert_eq!(package_names(&database).await, vec!["Mixed"]);

    // A batch the rule empties entirely leaves no package behind.
    let none = clipboard(&database, &[(GITHUB_TOO, "GitHub")]).await;
    assert!(none.is_empty());
    assert_eq!(package_names(&database).await, vec!["Mixed"]);
    assert_eq!(database.list_candidates().await.expect("list").len(), 1);
}

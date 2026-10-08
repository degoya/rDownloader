//! `groups` (RD-1170-02): one package per entry, the mirrors each group states, and the
//! budgets every group shares with the rule.

use serde_json::json;

use super::{
    Crawl, CrawlGroup, Executor, GroupLink, Limits, RunError,
    exec_tests::url,
    fakes::{Dns, Recorded, TestClock},
};
use crate::format::Rule;

const START: &str = "https://board.test/release/one";
const API: &str = "https://api.board.test/release/one";

/// The shape warez.cx answers in, cut down: two releases, each with its hoster lists in the
/// same part order, and a title for the whole page.
const RELEASES: &str = r#"{"title":"Show","releases":[
{"fulltitle":"Show.S01.720p","links":{"one.test":["https://one.test/a1","https://one.test/a2"],"two.test":["https://www.two.test/b1","https://www.two.test/b2"]}},
{"fulltitle":"Show.S01.1080p","links":{"two.test":["https://two.test/c1"],"one.test":["https://one.test/d1"]}}
]}"#;

/// A rule over `board.test` whose steps leave one entry per release in `releases`.
fn grouped(groups: serde_json::Value) -> Rule {
    let rule: Rule = serde_json::from_value(json!({
        "id": "board",
        "name": "board.test",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["board.test", "*.board.test"] },
        "steps": [
            { "kind": "fetch", "url": API, "into": "api" },
            { "kind": "regex", "from": "api",
              "pattern": "(\\{\"fulltitle\":\"[^\"]*\",\"links\":\\{[^}]*\\}\\})",
              "into": "releases", "all": true }
        ],
        "package": { "from": "regex", "pattern": "\"title\":\"([^\"]+)\"", "source": "api" },
        "groups": groups,
        "probe": START,
        "checked": "2026-10-07"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    rule
}

/// The group the warez.cx rule uses: every address of the entry, named by its full title.
fn per_release(mirrors: Option<&str>) -> serde_json::Value {
    let mut groups = json!({
        "from": "releases",
        "steps": [{ "kind": "regex", "from": "entry", "pattern": "\"(https://[^\"]+)\"",
                    "into": "links", "all": true }],
        "package": { "from": "regex", "pattern": "\"fulltitle\":\"([^\"]+)\"",
                     "source": "entry" }
    });
    if let Some(mirrors) = mirrors {
        groups["mirrors"] = mirrors.into();
    }
    groups
}

async fn run(rule: &Rule, fetcher: &Recorded) -> Result<Crawl, RunError> {
    let clock = TestClock::new();
    Executor::new(fetcher, &Dns::public(), &clock)
        .run(rule, &url(START))
        .await
}

async fn run_limited(rule: &Rule, fetcher: &Recorded, limits: Limits) -> Result<Crawl, RunError> {
    let clock = TestClock::new();
    Executor::new(fetcher, &Dns::public(), &clock)
        .with_limits(limits)
        .run(rule, &url(START))
        .await
}

fn link(url: &str, mirror: Option<u32>) -> GroupLink {
    GroupLink {
        url: url.to_owned(),
        mirror,
    }
}

#[tokio::test]
async fn every_release_becomes_a_package_and_its_hosters_mirrors_by_position() {
    let fetcher = Recorded::new().page(API, RELEASES);
    let crawl = run(&grouped(per_release(Some("by-host"))), &fetcher)
        .await
        .expect("crawled");
    assert_eq!(
        crawl.groups,
        [
            CrawlGroup {
                name: Some("Show.S01.720p".to_owned()),
                links: vec![
                    link("https://one.test/a1", Some(1)),
                    link("https://one.test/a2", Some(2)),
                    // `www.` is the same hoster: the second list pairs with the first.
                    link("https://www.two.test/b1", Some(1)),
                    link("https://www.two.test/b2", Some(2)),
                ],
            },
            CrawlGroup {
                name: Some("Show.S01.1080p".to_owned()),
                links: vec![
                    link("https://two.test/c1", Some(1)),
                    link("https://one.test/d1", Some(1)),
                ],
            },
        ]
    );
    // The flat list a caller that knows nothing of groups reads, and the page's own name.
    assert_eq!(crawl.links.len(), 6);
    assert_eq!(crawl.links[0], "https://one.test/a1");
    assert_eq!(crawl.links[5], "https://one.test/d1");
    assert_eq!(crawl.package_name.as_deref(), Some("Show"));
    assert!(!crawl.mirrors, "the page-wide flag stays off");
    assert_eq!(crawl.pages_fetched, 1);
}

#[tokio::test]
async fn a_group_without_mirrors_numbers_none_and_all_numbers_every_link_one() {
    let fetcher = Recorded::new().page(API, RELEASES);
    let crawl = run(&grouped(per_release(None)), &fetcher)
        .await
        .expect("crawled");
    assert!(
        crawl
            .groups
            .iter()
            .flat_map(|group| &group.links)
            .all(|link| link.mirror.is_none()),
        "{:?}",
        crawl.groups
    );
    let crawl = run(&grouped(per_release(Some("all"))), &fetcher)
        .await
        .expect("crawled");
    assert!(
        crawl
            .groups
            .iter()
            .flat_map(|group| &group.links)
            .all(|link| link.mirror == Some(1)),
        "{:?}",
        crawl.groups
    );
}

#[tokio::test]
async fn a_set_of_one_is_no_mirror() {
    // One hoster with two parts, the other with one: the second part has no copy.
    let fetcher = Recorded::new().page(
        API,
        r#"{"title":"Show","releases":[{"fulltitle":"Show.S02","links":{"one.test":["https://one.test/a1","https://one.test/a2"],"two.test":["https://two.test/b1"]}}]}"#,
    );
    let crawl = run(&grouped(per_release(Some("by-host"))), &fetcher)
        .await
        .expect("crawled");
    assert_eq!(
        crawl.groups[0].links,
        [
            link("https://one.test/a1", Some(1)),
            link("https://one.test/a2", None),
            link("https://two.test/b1", Some(1)),
        ]
    );
}

#[tokio::test]
async fn a_group_whose_source_reads_nothing_takes_the_rule_s_name() {
    let mut groups = per_release(None);
    groups["package"] = json!({ "from": "regex", "pattern": "\"season\":\"([^\"]+)\"",
                                "source": "entry" });
    let fetcher = Recorded::new().page(API, RELEASES);
    let crawl = run(&grouped(groups), &fetcher).await.expect("crawled");
    assert!(
        crawl
            .groups
            .iter()
            .all(|group| group.name.as_deref() == Some("Show")),
        "{:?}",
        crawl.groups
    );
}

#[tokio::test]
async fn a_link_two_groups_list_stays_with_the_first() {
    let fetcher = Recorded::new().page(
        API,
        r#"{"title":"Show","releases":[{"fulltitle":"A","links":{"one.test":["https://one.test/x"]}},{"fulltitle":"B","links":{"one.test":["https://one.test/x","https://one.test/y"]}}]}"#,
    );
    let crawl = run(&grouped(per_release(None)), &fetcher)
        .await
        .expect("crawled");
    assert_eq!(crawl.groups.len(), 2);
    assert_eq!(crawl.groups[0].links, [link("https://one.test/x", None)]);
    assert_eq!(crawl.groups[1].links, [link("https://one.test/y", None)]);
    assert_eq!(crawl.links, ["https://one.test/x", "https://one.test/y"]);
}

#[tokio::test]
async fn every_entry_starts_from_what_the_rule_left() {
    // Each entry first reads `last` and then overwrites it. The rule's own steps wrote it, so
    // the first entry reads the rule's value -- and so must the second: if it read what the
    // first entry wrote, it would yield a link of its own instead of a repeat.
    let fetcher = Recorded::new().page(
        API,
        r#"{"title":"Show","first":"https://one.test/rule","entries":["https://one.test/e1","https://one.test/e2"]}"#,
    );
    let rule: Rule = serde_json::from_value(json!({
        "id": "board", "name": "board.test", "group": "board", "version": 1,
        "match": { "hosts": ["board.test", "*.board.test"] },
        "steps": [
            { "kind": "fetch", "url": API, "into": "api" },
            { "kind": "regex", "from": "api", "pattern": "\"first\":\"([^\"]+)\"", "into": "last" },
            { "kind": "regex", "from": "api", "pattern": "\"(https://one\\.test/e\\d)\"",
              "into": "entries", "all": true }
        ],
        "package": { "from": "title" },
        "groups": {
            "from": "entries",
            "steps": [
                { "kind": "regex", "from": "last", "pattern": "(\\S+)", "into": "links" },
                { "kind": "regex", "from": "entry", "pattern": "(\\S+)", "into": "last" }
            ],
            "package": { "from": "variable", "name": "entry" }
        },
        "probe": START, "checked": "2026-10-07"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    let crawl = run(&rule, &fetcher).await.expect("crawled");
    // The second entry repeated the first one's link, so it is left out as empty.
    assert_eq!(
        crawl.groups,
        [CrawlGroup {
            name: Some("https://one.test/e1".to_owned()),
            links: vec![link("https://one.test/rule", None)],
        }]
    );
    assert_eq!(crawl.links, ["https://one.test/rule"]);
}

#[tokio::test]
async fn a_group_step_that_finds_nothing_refuses_the_run_under_its_own_number() {
    let mut groups = per_release(None);
    groups["steps"] = json!([{ "kind": "regex", "from": "entry", "pattern": "(magnet:\\S+)",
                               "into": "links" }]);
    let fetcher = Recorded::new().page(API, RELEASES);
    let refused = run(&grouped(groups), &fetcher)
        .await
        .expect_err("the page changed");
    // The rule has two steps, so the group's first is the third: numbered from 0, step 2.
    assert!(
        matches!(
            refused,
            RunError::Structure {
                step: 2,
                kind: "regex",
                ..
            }
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_group_that_writes_no_links_does_not_inherit_the_rule_s() {
    // The rule's own steps write `links`; the group's write `found`. No group may report the
    // rule's links as its own, so nothing is left and the run says so.
    let rule: Rule = serde_json::from_value(json!({
        "id": "board", "name": "board.test", "group": "board", "version": 1,
        "match": { "hosts": ["board.test", "*.board.test"] },
        "steps": [
            { "kind": "fetch", "url": API, "into": "api" },
            { "kind": "regex", "from": "api", "pattern": "\"(https://one\\.test/a1)\"",
              "into": "links" },
            { "kind": "regex", "from": "api",
              "pattern": "(\\{\"fulltitle\":\"[^\"]*\",\"links\":\\{[^}]*\\}\\})",
              "into": "releases", "all": true }
        ],
        "package": { "from": "title" },
        "groups": {
            "from": "releases",
            "steps": [{ "kind": "regex", "from": "entry", "pattern": "\"(https://[^\"]+)\"",
                        "into": "found", "all": true }],
            "package": { "from": "title" }
        },
        "probe": START, "checked": "2026-10-07"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    let fetcher = Recorded::new().page(API, RELEASES);
    let refused = run(&rule, &fetcher)
        .await
        .expect_err("no group wrote links");
    assert_eq!(refused, RunError::NoLinks);
}

#[tokio::test]
async fn more_entries_than_links_may_exist_is_refused_before_any_entry_runs() {
    let fetcher = Recorded::new().page(API, RELEASES);
    let limits = Limits {
        max_links: 1,
        ..Limits::default()
    };
    let refused = run_limited(&grouped(per_release(None)), &fetcher, limits)
        .await
        .expect_err("two entries, one link allowed");
    assert_eq!(refused, RunError::LimitLinks(1));
}

#[tokio::test]
async fn the_link_limit_counts_every_group_together() {
    let fetcher = Recorded::new().page(API, RELEASES);
    // Four links in the first group, two in the second: five is passed in the second.
    let limits = Limits {
        max_links: 5,
        ..Limits::default()
    };
    let refused = run_limited(&grouped(per_release(None)), &fetcher, limits)
        .await
        .expect_err("six links, five allowed");
    assert_eq!(refused, RunError::LimitLinks(5));
    let limits = Limits {
        max_links: 6,
        ..Limits::default()
    };
    run_limited(&grouped(per_release(None)), &fetcher, limits)
        .await
        .expect("six links, six allowed");
}

#[tokio::test]
async fn the_requests_of_every_group_count_against_the_run_s_pages() {
    let fetcher = Recorded::new()
        .page(API, r#"{"title":"Show","ids":["7","8","9"]}"#)
        .page("https://api.board.test/part/7", "https://one.test/seven")
        .page("https://api.board.test/part/8", "https://one.test/eight")
        .page("https://api.board.test/part/9", "https://one.test/nine");
    let rule: Rule = serde_json::from_value(json!({
        "id": "board", "name": "board.test", "group": "board", "version": 1,
        "match": { "hosts": ["board.test", "*.board.test"] },
        "steps": [
            { "kind": "fetch", "url": API, "into": "api" },
            { "kind": "regex", "from": "api", "pattern": "\"(\\d+)\"", "into": "ids", "all": true }
        ],
        "package": { "from": "title" },
        "groups": {
            "from": "ids",
            "into": "id",
            "steps": [
                { "kind": "fetch", "url": "https://api.board.test/part/${id}" },
                { "kind": "regex", "pattern": "(https://\\S+)", "into": "links" }
            ],
            "package": { "from": "variable", "name": "id" }
        },
        "probe": START, "checked": "2026-10-07"
    }))
    .expect("rule");
    let limits = Limits {
        max_pages: 3,
        ..Limits::default()
    };
    let refused = run_limited(&rule, &fetcher, limits)
        .await
        .expect_err("four requests, three allowed");
    assert_eq!(refused, RunError::LimitPages(3));
    assert_eq!(fetcher.requests().len(), 3);
}

#[tokio::test]
async fn a_rule_without_groups_answers_as_it_always_did() {
    let rule: Rule = serde_json::from_value(json!({
        "id": "board", "name": "board.test", "group": "board", "version": 1,
        "match": { "hosts": ["board.test"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://one\\.test/\\S+)", "into": "links", "all": true }
        ],
        "package": { "from": "title" },
        "mirrors": true,
        "probe": START, "checked": "2026-10-07"
    }))
    .expect("rule");
    let fetcher = Recorded::new().page(
        START,
        "<title>Show</title> https://one.test/a https://one.test/b",
    );
    let crawl = run(&rule, &fetcher).await.expect("crawled");
    assert_eq!(crawl.links, ["https://one.test/a", "https://one.test/b"]);
    assert!(crawl.groups.is_empty());
    assert!(crawl.mirrors);
}

//! `fetch` and `fetch-json` over a list (RD-180-18): one request per entry, not the first
//! entry only.

use serde_json::json;

use super::{
    Crawl, Executor, RunError,
    exec_tests::{from_variable, rule, url},
    fakes::{Dns, Recorded, TestClock},
};
use crate::format::Rule;

const START: &str = "https://board.test/release/one";

async fn run(rule: &Rule, fetcher: &Recorded, address: &str) -> Result<Crawl, RunError> {
    let clock = TestClock::new();
    Executor::new(fetcher, &Dns::public(), &clock)
        .run(rule, &url(address))
        .await
}

fn asked(fetcher: &Recorded) -> Vec<String> {
    fetcher
        .requests()
        .into_iter()
        .map(|request| request.url.to_string())
        .collect()
}

/// The shape of hide.cx as the owner measured it on 2026-10-01: the page carries no links,
/// the container answer names only link ids, and each id's own answer holds the address.
fn hide_cx() -> Rule {
    let rule: Rule = serde_json::from_value(json!({
        "id": "hide-cx",
        "name": "hide.cx",
        "group": "paste",
        "version": 1,
        "match": { "hosts": ["hide.cx", "*.hide.cx"] },
        "steps": [
            { "kind": "regex", "from": "url", "pattern": "/container/([0-9a-f-]+)",
              "into": "cid" },
            { "kind": "fetch", "url": "https://api.hide.cx/containers/${cid}",
              "into": "container" },
            { "kind": "regex", "from": "container",
              "pattern": "\\{\\s*\"id\"\\s*:\\s*\"([^\"]+)\"\\s*,\\s*\"host\"",
              "into": "lid", "all": true },
            { "kind": "fetch-json", "url": "https://api.hide.cx/containers/${cid}/links/${lid}",
              "path": "/url", "into": "links" }
        ],
        "package": { "from": "regex", "pattern": "\"title\"\\s*:\\s*\"([^\"]+)\"",
                     "source": "container" },
        "probe": "https://hide.cx/container/0a1b2c3d",
        "checked": "2026-10-01"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    rule
}

#[tokio::test]
async fn a_hide_cx_shaped_container_yields_every_link_in_order() {
    let rule = hide_cx();
    let container = r#"{"id":"0a1b2c3d","title":"Some.Release.2026","links":[
        {"id":"k1","host":"ddownload.com","file":"some.release.part1.rar"},
        {"id":"k2","host":"ddownload.com","file":"some.release.part2.rar"}]}"#;
    let fetcher = Recorded::new()
        .page("https://api.hide.cx/containers/0a1b2c3d", container)
        .page(
            "https://api.hide.cx/containers/0a1b2c3d/links/k1",
            r#"{"id":"k1","url":"https://ddownload.com/k7tsb1mdymxh"}"#,
        )
        .page(
            "https://api.hide.cx/containers/0a1b2c3d/links/k2",
            r#"{"id":"k2","url":"https://ddownload.com/p2q8wz0rt5ab"}"#,
        );
    let crawl = run(&rule, &fetcher, "https://hide.cx/container/0a1b2c3d")
        .await
        .expect("crawled");
    assert_eq!(
        crawl.links,
        [
            "https://ddownload.com/k7tsb1mdymxh",
            "https://ddownload.com/p2q8wz0rt5ab"
        ]
    );
    assert_eq!(crawl.package_name.as_deref(), Some("Some.Release.2026"));
    assert_eq!(
        asked(&fetcher),
        [
            "https://api.hide.cx/containers/0a1b2c3d",
            "https://api.hide.cx/containers/0a1b2c3d/links/k1",
            "https://api.hide.cx/containers/0a1b2c3d/links/k2"
        ]
    );
    assert_eq!(crawl.pages_fetched, 3);
}

#[tokio::test]
async fn fetch_over_a_list_keeps_one_body_per_entry_for_the_next_step() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "href=\"/part/([0-9]+)\"", "into": "parts",
              "all": true },
            // Relative on purpose: every entry resolves against the page the step started
            // from, not against the page the entry before it fetched.
            { "kind": "fetch", "url": "part/${parts}", "into": "bodies" },
            { "kind": "regex", "from": "bodies", "pattern": "(https://a\\.test/[^ ]+)",
              "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new()
        .page(
            "https://board.test/release/one",
            // Part 1 twice: a repeated entry is asked once, not refused as a cycle.
            "<a href=\"/part/1\"></a><a href=\"/part/2\"></a><a href=\"/part/1\"></a>",
        )
        .page("https://board.test/release/part/1", "https://a.test/one")
        .page("https://board.test/release/part/2", "https://a.test/two");
    let crawl = run(&rule, &fetcher, START).await.expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/one", "https://a.test/two"]);
    assert_eq!(
        asked(&fetcher),
        [
            START,
            "https://board.test/release/part/1",
            "https://board.test/release/part/2"
        ]
    );
}

#[tokio::test]
async fn two_list_placeholders_in_one_address_are_refused_before_any_request() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "a=([0-9]+)", "into": "a", "all": true },
            { "kind": "regex", "pattern": "b=([0-9]+)", "into": "b", "all": true },
            { "kind": "fetch-json", "url": "https://board.test/api/${a}/${b}", "path": "/url",
              "into": "links" }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page(START, "a=1 a=2 b=3 b=4");
    let refused = run(&rule, &fetcher, START).await.expect_err("refused");
    assert_eq!(refused.code(), "site_rules.structure");
    assert!(
        refused.to_string().contains("lists"),
        "the refusal names why: {refused}"
    );
    assert_eq!(asked(&fetcher), [START]);
}

#[tokio::test]
async fn one_value_expands_once_exactly_as_before() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "id=([0-9]+)", "into": "id" },
            { "kind": "fetch-json", "url": "https://board.test/api/${id}", "path": "/urls",
              "into": "links" }
        ]),
        from_variable("links"),
    );
    // A single match collapses to one value, so this is one request and not a list of one.
    let fetcher = Recorded::new().page(START, "id=7").page(
        "https://board.test/api/7",
        r#"{"urls":["https://a.test/1","https://a.test/2"]}"#,
    );
    let crawl = run(&rule, &fetcher, START).await.expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/1", "https://a.test/2"]);
    assert_eq!(asked(&fetcher), [START, "https://board.test/api/7"]);
}

#[tokio::test]
async fn an_empty_list_makes_no_request_and_ends_as_no_links() {
    // The same verdict an API answering `[]` gets: the page has nothing, the rule is intact.
    let rule = rule(
        json!([
            { "kind": "fetch-json", "url": "https://board.test/api/ids", "path": "/ids",
              "into": "ids" },
            { "kind": "fetch-json", "url": "https://board.test/api/link/${ids}", "path": "/url",
              "into": "links" }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/api/ids", r#"{"ids":[]}"#);
    let refused = run(&rule, &fetcher, START).await.expect_err("refused");
    assert_eq!(refused.code(), "site_rules.no_links");
    assert_eq!(asked(&fetcher), ["https://board.test/api/ids"]);
}

#[tokio::test]
async fn entries_are_siblings_and_a_list_longer_than_the_depth_limit_still_runs() {
    // Eight entries against a depth limit of six: each sits one request below the page the
    // step started from, so the list is a fan-out and not a chain.
    let ids: Vec<String> = (0..8).map(|id| id.to_string()).collect();
    let rule = rule(
        json!([
            { "kind": "fetch-json", "url": "https://board.test/api/ids", "path": "/ids",
              "into": "ids" },
            { "kind": "fetch-json", "url": "https://board.test/api/link/${ids}", "path": "/url",
              "into": "links" }
        ]),
        from_variable("links"),
    );
    let mut fetcher = Recorded::new().page(
        "https://board.test/api/ids",
        &json!({ "ids": ids }).to_string(),
    );
    for id in &ids {
        fetcher = fetcher.page(
            &format!("https://board.test/api/link/{id}"),
            &json!({ "url": format!("https://a.test/{id}") }).to_string(),
        );
    }
    let crawl = run(&rule, &fetcher, START).await.expect("crawled");
    assert_eq!(crawl.links.len(), 8);
    assert_eq!(
        crawl.links.first().map(String::as_str),
        Some("https://a.test/0")
    );
    assert_eq!(
        crawl.links.last().map(String::as_str),
        Some("https://a.test/7")
    );
    assert_eq!(crawl.pages_fetched, 9);
}

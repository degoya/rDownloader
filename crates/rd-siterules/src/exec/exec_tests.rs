//! The seven steps, one by one, and what a run does at its edges.

use std::sync::Arc;

use serde_json::json;
use url::Url;

use super::{
    Crawl, Executor, RunError,
    fakes::{Broker, Dns, Recorded, RecordingBroker, TestClock},
    ports::{FetchFailure, FetchResponse, Method},
};
use crate::format::Rule;

/// A rule over `board.test`, with the steps and package source a test cares about.
pub(super) fn rule(steps: serde_json::Value, package: serde_json::Value) -> Rule {
    let rule: Rule = serde_json::from_value(json!({
        "id": "board",
        "name": "board.test",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["board.test", "*.board.test"] },
        "dead": ["old-board.test"],
        "steps": steps,
        "package": package,
        "probe": "https://board.test/release/one",
        "checked": "2026-09-21"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    rule
}

pub(super) fn from_variable(name: &str) -> serde_json::Value {
    json!({ "from": "variable", "name": name })
}

pub(super) fn url(text: &str) -> Url {
    Url::parse(text).expect("url")
}

/// One run with the default limits, a public resolver and no captcha broker.
async fn run(rule: &Rule, fetcher: &Recorded, address: &str) -> Result<Crawl, RunError> {
    let clock = TestClock::new();
    Executor::new(fetcher, &Dns::public(), &clock)
        .run(rule, &url(address))
        .await
}

#[tokio::test]
async fn fetch_and_two_regex_steps_yield_the_links_and_the_title() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "<div class=\"links\">(.+?)</div>", "into": "container" },
            { "kind": "regex", "from": "container", "pattern": "href=\"(https?://[^\"]+)\"",
              "into": "links", "all": true }
        ]),
        json!({ "from": "title" }),
    );
    let fetcher = Recorded::new().page(
        "https://board.test/release/one",
        "<title>  Some   Release 2026 </title><div class=\"links\">\
         href=\"https://host1.test/a\" href=\"https://host2.test/b\"</div>",
    );
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(
        crawl.links,
        ["https://host1.test/a", "https://host2.test/b"]
    );
    assert_eq!(crawl.package_name.as_deref(), Some("Some Release 2026"));
    assert_eq!(crawl.pages_fetched, 1);
}

#[tokio::test]
async fn fetch_json_takes_the_value_its_pointer_names() {
    let rule = rule(
        json!([{ "kind": "fetch-json", "url": "https://board.test/api/1", "path": "/data/items",
                 "into": "links" }]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page(
        "https://board.test/api/1",
        r#"{"data":{"items":["https://a.test/1","https://a.test/2"]}}"#,
    );
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/1", "https://a.test/2"]);
    assert_eq!(crawl.package_name.as_deref(), Some("https://a.test/1"));
}

#[tokio::test]
async fn a_pointer_that_names_nothing_is_a_structure_refusal() {
    let rule = rule(
        json!([{ "kind": "fetch-json", "url": "https://board.test/api/1", "path": "/missing",
                 "into": "links" }]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/api/1", r#"{"data":[]}"#);
    let refused = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.structure");
}

#[tokio::test]
async fn decode_runs_over_every_element_of_a_list() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "data-u=\"([^\"]+)\"", "into": "raw", "all": true },
            { "kind": "decode", "encoding": "base64", "from": "raw", "into": "links" }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page(
        "https://board.test/release/one",
        "<a data-u=\"aHR0cHM6Ly9hLnRlc3QvMQ==\"></a><a data-u=\"aHR0cHM6Ly9hLnRlc3QvMg==\"></a>",
    );
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/1", "https://a.test/2"]);
}

#[tokio::test]
async fn what_cannot_be_decoded_is_an_honest_refusal_and_not_a_javascript_interpreter() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "<script>(.+?)</script>", "into": "raw" },
            { "kind": "decode", "encoding": "js-string", "from": "raw", "into": "links" }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page(
        "https://board.test/release/one",
        "<script>document.write(atob(window.k))</script>",
    );
    let refused = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.decode_failed");
    assert!(!refused.not_mine());
}

#[tokio::test]
async fn a_form_is_posted_with_its_fields_expanded() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "value=\"([a-z0-9]+)\"", "into": "token" },
            { "kind": "form", "url": "https://board.test/go", "fields": { "t": "${token}" },
              "into": "result" },
            { "kind": "regex", "from": "result", "pattern": "(https://[^ ]+)", "into": "links",
              "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new()
        .page(
            "https://board.test/release/one",
            "<input name=\"t\" value=\"abc123\">",
        )
        .page("https://board.test/go", "https://a.test/final");
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/final"]);
    let posted = fetcher
        .requests()
        .into_iter()
        .find(|request| request.method == Method::Post)
        .expect("a form was posted");
    assert_eq!(posted.url.as_str(), "https://board.test/go");
    assert_eq!(posted.form.get("t").map(String::as_str), Some("abc123"));
}

#[tokio::test]
async fn a_redirect_step_takes_the_target_without_fetching_it() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "href=\"(/out/[0-9]+)\"", "into": "raw", "all": true },
            { "kind": "redirect", "from": "raw", "into": "links" }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new()
        .page(
            "https://board.test/release/one",
            "<a href=\"/out/1\"></a><a href=\"/out/2\"></a>",
        )
        .redirect("https://board.test/out/1", 302, "https://a.test/one")
        .redirect("https://board.test/out/2", 301, "https://a.test/two");
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/one", "https://a.test/two"]);
    let asked: Vec<String> = fetcher
        .requests()
        .into_iter()
        .map(|request| request.url.to_string())
        .collect();
    assert!(
        !asked.iter().any(|url| url.starts_with("https://a.test")),
        "the target of a redirect step must not be fetched: {asked:?}"
    );
}

#[tokio::test]
async fn a_redirect_during_a_fetch_is_followed_and_the_target_answers() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new()
        .redirect("https://board.test/release/one", 302, "/release/one-moved")
        .page("https://board.test/release/one-moved", "https://a.test/x");
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/x"]);
    assert_eq!(crawl.pages_fetched, 2);
}

#[tokio::test]
async fn a_captcha_step_asks_the_broker_and_submits_its_token() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "data-sitekey=\"([^\"]+)\"", "into": "sitekey" },
            { "kind": "captcha", "challenge": "recaptcha-v2", "sitekey": "${sitekey}",
              "into": "answer" },
            { "kind": "form", "url": "https://board.test/unlock",
              "fields": { "g-recaptcha-response": "${answer}" }, "into": "result" },
            { "kind": "regex", "from": "result", "pattern": "(https://[^ ]+)", "into": "links",
              "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new()
        .page(
            "https://board.test/release/one",
            "<div data-sitekey=\"6Lx-KEY\"></div>",
        )
        .page("https://board.test/unlock", "https://a.test/unlocked");
    let broker = RecordingBroker::default();
    let clock = TestClock::new();
    let crawl = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_captcha(&broker)
        .run(&rule, &url("https://board.test/release/one"))
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/unlocked"]);
    let asked = broker.seen.lock().expect("lock").clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].challenge, "recaptcha-v2");
    assert_eq!(asked[0].sitekey.as_deref(), Some("6Lx-KEY"));
    assert_eq!(asked[0].page_url.as_str(), "https://board.test/release/one");
    let posted = fetcher
        .requests()
        .into_iter()
        .find(|request| request.method == Method::Post)
        .expect("posted");
    assert_eq!(
        posted.form.get("g-recaptcha-response").map(String::as_str),
        Some("token-42")
    );
}

#[tokio::test]
async fn a_captcha_refuses_with_its_own_code_when_nobody_answers() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "captcha", "challenge": "recaptcha-v2" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/release/one", "https://a.test/x");
    // No broker at all.
    let refused = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.captcha_failed");
    // A broker that cannot answer.
    let clock = TestClock::new();
    let broker = Broker(Err("no service is configured".to_owned()));
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_captcha(&broker)
        .run(&rule, &url("https://board.test/release/one"))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.captcha_failed");
}

#[tokio::test]
async fn an_address_the_rule_does_not_claim_is_the_one_refusal_that_keeps_the_search_going() {
    let rule = rule(json!([{ "kind": "fetch" }]), from_variable("links"));
    let fetcher = Recorded::new();
    let refused = run(&rule, &fetcher, "https://elsewhere.test/x")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.not_claimed");
    assert!(refused.not_mine());
    assert!(fetcher.requests().is_empty());
}

#[tokio::test]
async fn a_dead_host_is_revived_onto_the_canonical_one() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/release/one", "https://a.test/x");
    let crawl = run(&rule, &fetcher, "https://old-board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.address.as_str(), "https://board.test/release/one");
    assert_eq!(crawl.links, ["https://a.test/x"]);
}

#[tokio::test]
async fn a_pattern_that_matches_nothing_says_the_structure_changed() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "<div class=\"links\">(.+?)</div>", "into": "links" }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/release/one", "<p>a new theme</p>");
    let refused = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.structure");
    assert!(!refused.not_mine());
}

#[tokio::test]
async fn an_empty_result_is_a_refusal_with_a_code_and_not_an_empty_package() {
    // Nothing in the list at all.
    let rule = rule(
        json!([{ "kind": "fetch-json", "url": "https://board.test/api/1", "path": "/items",
                 "into": "links" }]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/api/1", r#"{"items":[]}"#);
    let refused = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.no_links");
    // Entries that survive nothing: a scheme no downloader can use, and an address that
    // points back at this machine.
    let fetcher = Recorded::new().page(
        "https://board.test/api/1",
        r#"{"items":["javascript:void(0)","http://127.0.0.1:8710/api/v1/system"]}"#,
    );
    let refused = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.no_links");
}

#[tokio::test]
async fn a_dead_page_a_guarded_page_and_a_broken_one_are_told_apart() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let address = "https://board.test/release/one";
    for (answer, expected) in [
        (
            Ok(FetchResponse {
                status: 404,
                headers: Vec::new(),
                body: String::new(),
            }),
            "site_rules.page_dead",
        ),
        (
            Ok(FetchResponse {
                status: 403,
                headers: Vec::new(),
                body: String::new(),
            }),
            "site_rules.blocked",
        ),
        (
            Ok(FetchResponse {
                status: 503,
                headers: Vec::new(),
                body: String::new(),
            }),
            "site_rules.fetch_failed",
        ),
        (
            Err(FetchFailure::Unreachable("no route to host".to_owned())),
            "site_rules.page_dead",
        ),
        (Err(FetchFailure::Timeout), "site_rules.fetch_failed"),
    ] {
        let fetcher = Recorded::new().answer(address, answer);
        let refused = run(&rule, &fetcher, address).await.expect_err("refused");
        assert_eq!(refused.code(), expected);
    }
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_a_dead_page() {
    let rule = rule(json!([{ "kind": "fetch" }]), from_variable("links"));
    let fetcher = Recorded::new();
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public().missing("board.test"), &clock)
        .run(&rule, &url("https://board.test/release/one"))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.page_dead");
    assert!(fetcher.requests().is_empty());
}

#[tokio::test]
async fn a_package_source_that_finds_nothing_costs_the_name_and_not_the_links() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        json!({ "from": "regex", "pattern": "<h1>(.+?)</h1>" }),
    );
    let fetcher = Recorded::new().page("https://board.test/release/one", "https://a.test/x");
    let crawl = run(&rule, &fetcher, "https://board.test/release/one")
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/x"]);
    assert_eq!(crawl.package_name, None);
}

#[tokio::test]
async fn the_clock_port_is_the_only_time_the_executor_reads() {
    // A run whose every request is free finishes however small the budget is; the limit test
    // that spends it uses the same clock, which is what makes that test instant.
    let clock = Arc::new(TestClock::new());
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page("https://board.test/release/one", "https://a.test/x");
    let dns = Dns::public();
    let executor = Executor::new(&fetcher, &dns, clock.as_ref()).with_limits(super::Limits {
        max_total_time: std::time::Duration::from_millis(1),
        ..super::Limits::default()
    });
    executor
        .run(&rule, &url("https://board.test/release/one"))
        .await
        .expect("crawled");
}

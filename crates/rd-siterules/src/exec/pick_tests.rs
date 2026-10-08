//! Two-stage rules (RD-1170-03): the first stage lists the entries and fetches nothing behind
//! them, the second resolves one chosen entry -- with one captcha for that entry -- and a
//! captcha a person takes long over is not counted against the run's budget.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;

use super::{
    CaptchaRequest, CaptchaSolver, CrawlGroup, Executor, GroupLink, Method, RunError,
    exec_tests::url,
    fakes::{Broker, Dns, Recorded, RecordingBroker, TestClock},
};
use crate::format::Rule;

const PAGE: &str = "https://series.test/serie/show/";
const API: &str = "https://series.test/api/media/m1/releases";
const DOWNLOADS: &str = "https://series.test/api/releases/r2/downloads/hoster-one";
const DEVICE: &str = "0123456789abcdef0123456789abcdef";

const HTML: &str = r#"<div id="list" data-mediaid="m1" data-mediatitle="The Show"
 data-captchasitekey="site-key-1"></div>"#;

/// Three releases the way serienjunkies.org answers: two episodes and a season pack.
const RELEASES: &str = r#"{"S1":{"items":[
{"_id":"r1","name":"The.Show.S01E01.720p","season":1,"episode":1,"resolution":"720p","language":"GERMAN","hoster":["hoster-one"]},
{"_id":"r2","name":"The.Show.S01E02.1080p","season":1,"episode":2,"resolution":"1080p","language":"GERMAN","hoster":["hoster-one"]},
{"_id":"r3","name":"The.Show.S01.720p","season":1,"episode":null,"resolution":"720p","language":"ENGLISH","hoster":["hoster-one"]}
]}}"#;

/// The answer for one release, with the slashes escaped the way some APIs write them.
const LINKS: &str =
    r#"[{"url":"https://hoster-one.test/a"},{"url":"https:\/\/hoster-one.test\/b"}]"#;

fn two_stage() -> Rule {
    let rule: Rule = serde_json::from_value(json!({
        "id": "series",
        "name": "series.test",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["series.test"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "data-mediaid=\"([0-9a-z]+)\"", "into": "media" },
            { "kind": "regex", "pattern": "data-captchasitekey=\"([^\"]+)\"", "into": "sitekey" },
            { "kind": "fetch", "url": "https://series.test/api/media/${media}/releases",
              "into": "api" },
            { "kind": "regex", "from": "api", "pattern": "(\\{\"_id\":\"[^{}]*\\})",
              "into": "releases", "all": true }
        ],
        "package": { "from": "regex", "pattern": "data-mediatitle=\"([^\"]+)\"" },
        "groups": {
            "from": "releases",
            "pick": { "attributes": {
                "season": "\"season\":(\\d+)",
                "episode": "\"episode\":(\\d+)",
                "resolution": "\"resolution\":\"([^\"]+)\"",
                "language": "\"language\":\"([^\"]+)\"",
                "hoster": "\"hoster\":\\[\"([^\"]+)\""
            } },
            "steps": [
                { "kind": "regex", "from": "entry", "pattern": "\"_id\":\"([0-9a-z]+)\"",
                  "into": "release" },
                { "kind": "regex", "from": "entry", "pattern": "\"hoster\":\\[\"([^\"]+)\"",
                  "into": "hoster" },
                { "kind": "captcha", "challenge": "recaptcha-v2", "sitekey": "${sitekey}",
                  "page": "${url}", "invisible": true, "into": "token" },
                { "kind": "form",
                  "url": "https://series.test/api/releases/${release}/downloads/${hoster}",
                  "fields": { "recaptchaToken": "${token}", "fphash": "${device_id}" },
                  "json": true, "into": "answer" },
                { "kind": "regex", "from": "answer", "pattern": "\"url\"\\s*:\\s*(\"[^\"]+\")",
                  "into": "quoted", "all": true },
                { "kind": "decode", "encoding": "js-string", "from": "quoted", "into": "links" }
            ],
            "package": { "from": "regex", "pattern": "\"name\":\"([^\"]+)\"", "source": "entry" }
        },
        "probe": PAGE,
        "checked": "2026-10-07"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    rule
}

fn network() -> Recorded {
    Recorded::new()
        .page(PAGE, HTML)
        .page(API, RELEASES)
        .page(DOWNLOADS, LINKS)
}

#[tokio::test]
async fn the_first_stage_lists_every_entry_and_fetches_nothing_behind_them() {
    let fetcher = network();
    let broker = RecordingBroker::default();
    let clock = TestClock::new();
    let crawl = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_captcha(&broker)
        .run(&two_stage(), &url(PAGE))
        .await
        .expect("listed");
    assert!(crawl.links.is_empty() && crawl.groups.is_empty());
    assert_eq!(crawl.package_name.as_deref(), Some("The Show"));
    assert_eq!(
        crawl.pages_fetched, 2,
        "the page and the release list, nothing else"
    );
    assert!(
        broker.seen.lock().expect("seen").is_empty(),
        "no captcha yet"
    );
    let list = crawl.pick.expect("a list to choose from");
    let labels: Vec<_> = list
        .entries
        .iter()
        .map(|entry| entry.label.as_deref())
        .collect();
    assert_eq!(
        labels,
        [
            Some("The.Show.S01E01.720p"),
            Some("The.Show.S01E02.1080p"),
            Some("The.Show.S01.720p")
        ]
    );
    let first = &list.entries[0].attributes;
    assert_eq!(first.get("season").map(String::as_str), Some("1"));
    assert_eq!(first.get("episode").map(String::as_str), Some("1"));
    assert_eq!(first.get("resolution").map(String::as_str), Some("720p"));
    assert_eq!(first.get("hoster").map(String::as_str), Some("hoster-one"));
    // A season pack has no episode: the attribute is absent, not empty.
    assert_eq!(list.entries[2].attributes.get("episode"), None);
    assert_eq!(
        list.entries[2]
            .attributes
            .get("language")
            .map(String::as_str),
        Some("ENGLISH")
    );
    // What the second stage starts from: the variables the rule's own steps wrote.
    assert_eq!(
        list.variables
            .get("sitekey")
            .and_then(|value| value.first()),
        Some("site-key-1")
    );
    assert_eq!(list.variables.get("entry"), None);
}

#[tokio::test]
async fn the_second_stage_resolves_one_entry_with_one_captcha_and_a_json_request() {
    let fetcher = network();
    let broker = RecordingBroker::default();
    let clock = TestClock::new();
    let dns = Dns::public();
    let executor = Executor::new(&fetcher, &dns, &clock)
        .with_captcha(&broker)
        .with_device_id(DEVICE);
    let rule = two_stage();
    let crawl = executor.run(&rule, &url(PAGE)).await.expect("listed");
    let list = crawl.pick.expect("list");
    let group = executor
        .resolve(&rule, &crawl.address, &list, 1)
        .await
        .expect("resolved");
    assert_eq!(
        group,
        CrawlGroup {
            name: Some("The.Show.S01E02.1080p".to_owned()),
            links: vec![
                GroupLink {
                    url: "https://hoster-one.test/a".to_owned(),
                    mirror: None,
                },
                GroupLink {
                    url: "https://hoster-one.test/b".to_owned(),
                    mirror: None,
                },
            ],
        }
    );
    // One challenge, for this entry, on the series page rather than on the API's answer.
    let seen = broker.seen.lock().expect("seen").clone();
    assert_eq!(
        seen,
        [CaptchaRequest {
            challenge: "recaptcha-v2".to_owned(),
            sitekey: Some("site-key-1".to_owned()),
            page_url: url(PAGE),
            invisible: true,
        }]
    );
    let requests = fetcher.requests();
    let post = requests.last().expect("the download request");
    assert_eq!(post.url.as_str(), DOWNLOADS);
    assert_eq!(post.method, Method::Post);
    assert!(post.json, "sent as JSON");
    assert_eq!(
        post.form.get("recaptchaToken").map(String::as_str),
        Some("token-42")
    );
    assert_eq!(post.form.get("fphash").map(String::as_str), Some(DEVICE));
    // The first stage's requests carried no body; only the download request is JSON.
    assert!(
        requests[..requests.len() - 1]
            .iter()
            .all(|request| !request.json)
    );
}

#[tokio::test]
async fn an_unanswered_captcha_or_an_unknown_entry_is_refused_by_its_own_code() {
    let fetcher = network();
    let clock = TestClock::new();
    let rule = two_stage();
    let refusing = Broker(Err("captcha.timeout".to_owned()));
    let dns = Dns::public();
    let executor = Executor::new(&fetcher, &dns, &clock).with_captcha(&refusing);
    let crawl = executor.run(&rule, &url(PAGE)).await.expect("listed");
    let list = crawl.pick.expect("list");
    let error = executor
        .resolve(&rule, &crawl.address, &list, 0)
        .await
        .expect_err("no answer");
    assert_eq!(error.code(), "site_rules.captcha_failed");
    let error = executor
        .resolve(&rule, &crawl.address, &list, 3)
        .await
        .expect_err("no such entry");
    assert_eq!(error, RunError::NoEntry(3));
}

/// A broker a person takes ten minutes to answer, on the run's own clock.
struct SlowBroker(Arc<TestClock>);

#[async_trait]
impl CaptchaSolver for SlowBroker {
    async fn solve(&self, _request: CaptchaRequest) -> Result<String, String> {
        self.0.advance(Duration::from_secs(600));
        Ok("token-slow".to_owned())
    }
}

#[tokio::test]
async fn a_person_solving_slowly_does_not_spend_the_second_stage_s_budget() {
    let fetcher = network();
    let clock = Arc::new(TestClock::new());
    let broker = SlowBroker(Arc::clone(&clock));
    let rule = two_stage();
    let dns = Dns::public();
    let executor = Executor::new(&fetcher, &dns, clock.as_ref())
        .with_captcha(&broker)
        .with_device_id(DEVICE);
    let crawl = executor.run(&rule, &url(PAGE)).await.expect("listed");
    let list = crawl.pick.expect("list");
    let group = executor
        .resolve(&rule, &crawl.address, &list, 1)
        .await
        .expect("ten minutes of captcha are not the run's ninety seconds");
    assert_eq!(group.links.len(), 2);

    // A one-stage rule keeps counting the wait, exactly as it did before.
    let mut one_stage: serde_json::Value = serde_json::to_value(&rule).expect("encode");
    let group_steps = one_stage["groups"]["steps"].clone();
    let mut steps = one_stage["steps"].as_array().cloned().expect("steps");
    steps.push(json!({ "kind": "regex", "from": "releases",
                       "pattern": "(\\{\"_id\":\"r2\"[^{}]*\\})", "into": "entry" }));
    steps.extend(group_steps.as_array().cloned().expect("group steps"));
    one_stage["steps"] = steps.into();
    one_stage.as_object_mut().expect("object").remove("groups");
    let one_stage: Rule = serde_json::from_value(one_stage).expect("rule");
    one_stage.validate().expect("valid");
    let error = executor
        .run(&one_stage, &url(PAGE))
        .await
        .expect_err("the wait counts");
    assert!(matches!(error, RunError::LimitTime(_)), "{error:?}");
}

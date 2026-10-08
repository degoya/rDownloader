//! RD-1190-22: what a rule hands on is held to the rule. The captcha page goes to the solver and
//! to the person's browser, so it is one of the rule's own hosts; and the package name a page
//! returned is cut to what a package name may be.

use serde_json::json;

use super::{
    Executor,
    exec_tests::{from_variable, rule, url},
    fakes::{Dns, Recorded, RecordingBroker, TestClock},
    steps::MAX_PACKAGE_NAME_CHARS,
};

const START: &str = "https://board.test/release/one";

#[tokio::test]
async fn a_captcha_page_outside_the_rules_hosts_never_reaches_the_solver() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "captcha", "challenge": "recaptcha-v2", "sitekey": "6Lx-KEY",
              "page": "https://victim.test/signup", "into": "answer" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        from_variable("links"),
    );
    let fetcher = Recorded::new().page(START, "https://a.test/x");
    let broker = RecordingBroker::default();
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_captcha(&broker)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.target_not_allowed");
    assert!(broker.seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_package_name_is_cut_to_what_the_queue_takes() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ]),
        json!({ "from": "title" }),
    );
    let body = format!(
        "<title>{}</title> https://a.test/x",
        "Release ".repeat(MAX_PACKAGE_NAME_CHARS)
    );
    let fetcher = Recorded::new().page(START, &body);
    let clock = TestClock::new();
    let crawl = Executor::new(&fetcher, &Dns::public(), &clock)
        .run(&rule, &url(START))
        .await
        .expect("crawled");
    let name = crawl.package_name.expect("a name");
    assert!(name.chars().count() <= MAX_PACKAGE_NAME_CHARS, "{name}");
    assert!(name.starts_with("Release Release"), "{name}");
    assert!(!name.ends_with(' '), "{name:?}");
}

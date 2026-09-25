//! The four verdicts, each against a server that plays the behaviour.
//!
//! The server is the recorded fetcher the executor's own tests use: a port with a table of
//! answers rather than a socket. That is deliberate and it is what the job means by "a local
//! server" — the self-test reaches the network by design, so no test of it may, or the suite
//! would depend on a board still being up.

use serde_json::json;

use super::{PROBE_INVALID, RuleReport, Verdict, check};
use crate::{
    exec::{
        Executor, RunError,
        fakes::{Dns, Recorded, TestClock},
        ports::{FetchFailure, FetchResponse},
    },
    format::Rule,
};

const PROBE: &str = "https://board.test/release/one";

/// A rule whose probe is a page with a container of links.
fn rule() -> Rule {
    let rule: Rule = serde_json::from_value(json!({
        "id": "board",
        "name": "board.test",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["board.test"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "<div class=\"links\">(.+?)</div>", "into": "container" },
            { "kind": "regex", "from": "container", "pattern": "href=\"(https?://[^\"]+)\"",
              "into": "links", "all": true }
        ],
        "package": { "from": "title" },
        "probe": PROBE,
        "checked": "2026-09-21"
    }))
    .expect("rule");
    rule.validate().expect("valid");
    rule
}

async fn report(fetcher: &Recorded) -> RuleReport {
    let clock = TestClock::new();
    let dns = Dns::public();
    let executor = Executor::new(fetcher, &dns, &clock);
    check(&executor, &rule()).await
}

#[tokio::test]
async fn a_rule_that_still_finds_its_links_is_ok() {
    let fetcher = Recorded::new().page(
        PROBE,
        "<title>Some Release</title><div class=\"links\">\
         href=\"https://host1.test/a\" href=\"https://host2.test/b\"</div>",
    );
    let report = report(&fetcher).await;
    assert_eq!(report.verdict, Verdict::Ok);
    assert_eq!(report.reason, None);
    assert_eq!(report.links, 2);
    assert_eq!(report.pages, 1);
    assert_eq!(report.rule_id, "board");
    assert_eq!(report.probe, PROBE);
}

#[tokio::test]
async fn a_page_that_answers_but_changed_its_layout_is_structural() {
    let fetcher = Recorded::new().page(
        PROBE,
        "<title>Some Release</title><section class=\"downloads\">nothing the rule knows</section>",
    );
    let report = report(&fetcher).await;
    assert_eq!(report.verdict, Verdict::Structural);
    assert_eq!(report.reason, Some("site_rules.structure"));
    assert_eq!(report.links, 0);
}

#[tokio::test]
async fn a_page_that_guards_itself_is_blocked() {
    let fetcher = Recorded::new().answer(
        PROBE,
        Ok(FetchResponse {
            status: 403,
            headers: Vec::new(),
            body: String::new(),
        }),
    );
    let report = report(&fetcher).await;
    assert_eq!(report.verdict, Verdict::Blocked);
    assert_eq!(report.reason, Some("site_rules.blocked"));
}

#[tokio::test]
async fn a_service_that_no_longer_answers_is_dead() {
    let fetcher = Recorded::new().answer(
        PROBE,
        Err(FetchFailure::Unreachable("no such host".to_owned())),
    );
    let report = report(&fetcher).await;
    assert_eq!(report.verdict, Verdict::Dead);
    assert_eq!(report.reason, Some("site_rules.page_dead"));
}

/// A 410 is the other way a page dies: it answers, and what it answers is "gone".
#[tokio::test]
async fn a_page_that_answers_gone_is_dead_as_well() {
    let fetcher = Recorded::new().answer(
        PROBE,
        Ok(FetchResponse {
            status: 410,
            headers: Vec::new(),
            body: String::new(),
        }),
    );
    assert_eq!(report(&fetcher).await.verdict, Verdict::Dead);
}

#[tokio::test]
async fn a_probe_that_is_not_an_address_is_a_finding_about_the_rule() {
    let mut rule = rule();
    rule.probe = "board.test/release/one".to_owned();
    let fetcher = Recorded::new();
    let clock = TestClock::new();
    let dns = Dns::public();
    let executor = Executor::new(&fetcher, &dns, &clock);
    let report = check(&executor, &rule).await;
    assert_eq!(report.verdict, Verdict::Structural);
    assert_eq!(report.reason, Some(PROBE_INVALID));
    assert!(fetcher.requests().is_empty(), "nothing was fetched");
}

#[test]
fn every_refusal_sorts_into_one_of_the_four_states() {
    let cases = [
        (RunError::NotClaimed("u".into()), Verdict::Structural),
        (
            RunError::PageDead {
                url: "u".into(),
                reason: "404".into(),
            },
            Verdict::Dead,
        ),
        (
            RunError::Blocked {
                url: "u".into(),
                status: 403,
            },
            Verdict::Blocked,
        ),
        (
            RunError::FetchFailed {
                url: "u".into(),
                reason: "tls".into(),
            },
            Verdict::Dead,
        ),
        (
            RunError::TargetNotAllowed { url: "u".into() },
            Verdict::Structural,
        ),
        (
            RunError::AddressNotPublic {
                host: "h".into(),
                address: "127.0.0.1".parse().expect("address"),
            },
            Verdict::Dead,
        ),
        (
            RunError::ResponseTooLarge {
                url: "u".into(),
                limit: 1,
            },
            Verdict::Structural,
        ),
        (
            RunError::Structure {
                step: 0,
                kind: "regex",
                detail: "x".into(),
            },
            Verdict::Structural,
        ),
        (
            RunError::DecodeFailed {
                step: 0,
                encoding: "hex".into(),
            },
            Verdict::Structural,
        ),
        (
            RunError::CaptchaFailed {
                step: 0,
                reason: "x".into(),
            },
            Verdict::Blocked,
        ),
        (RunError::Cycle { url: "u".into() }, Verdict::Structural),
        (RunError::LimitDepth(1), Verdict::Structural),
        (RunError::LimitPages(1), Verdict::Structural),
        (RunError::LimitLinks(1), Verdict::Structural),
        (RunError::LimitTime(1), Verdict::Structural),
        (RunError::NoLinks, Verdict::Structural),
    ];
    for (error, expected) in &cases {
        assert_eq!(Verdict::of(error), *expected, "{}", error.code());
    }
}

/// The stored word and the translated code are both stable, and neither is shared.
#[test]
fn every_verdict_has_its_own_word_and_its_own_code() {
    let all = [
        Verdict::Ok,
        Verdict::Structural,
        Verdict::Blocked,
        Verdict::Dead,
    ];
    let mut words: Vec<_> = all.iter().map(|verdict| verdict.as_str()).collect();
    words.sort_unstable();
    words.dedup();
    assert_eq!(words.len(), all.len());
    let mut codes: Vec<_> = all.iter().map(|verdict| verdict.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
    assert!(
        codes
            .iter()
            .all(|code| code.starts_with("site_rules.state."))
    );
    for verdict in all {
        assert_eq!(Verdict::parse(verdict.as_str()), Some(verdict));
    }
    assert_eq!(Verdict::parse("degraded"), None);
    assert!(Verdict::Ok.is_ok());
    assert!(!Verdict::Structural.is_ok());
}

/// A rule the self-test cannot check is a rule nobody is watching, so both fields are
/// required rather than optional: a body without them does not become a rule at all.
#[test]
fn a_rule_without_a_probe_or_a_check_date_is_refused() {
    for field in ["probe", "checked"] {
        let mut body = serde_json::to_value(rule()).expect("encode");
        body.as_object_mut().expect("object").remove(field);
        let refused = serde_json::from_value::<Rule>(body);
        assert!(refused.is_err(), "{field} is required");
    }
}

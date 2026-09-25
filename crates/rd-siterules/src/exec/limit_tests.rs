//! The bolts and the budgets. Every test here would hang, leak or run away without the
//! limit it proves: a redirect onto a foreign host, a name pointing into the local network,
//! a redirect loop, a chain with no end, a fan-out with no end, a body with no end.

use std::{sync::Arc, time::Duration};

use serde_json::json;

use super::{
    Executor, Limits,
    exec_tests::{from_variable, rule, url},
    fakes::{Dns, FlippingDns, Recorded, TestClock},
    ports::{FetchFailure, FetchResponse},
};
use crate::format::Rule;

const START: &str = "https://board.test/release/one";

fn collecting(mut steps: Vec<serde_json::Value>) -> Rule {
    steps.push(
        json!({ "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links",
                       "all": true }),
    );
    rule(serde_json::Value::Array(steps), from_variable("links"))
}

#[tokio::test]
async fn a_rule_cannot_fetch_a_host_its_match_does_not_name() {
    let rule = collecting(vec![
        json!({ "kind": "fetch", "url": "https://evil.test/x" }),
    ]);
    let fetcher = Recorded::new().page("https://evil.test/x", "https://a.test/x");
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.target_not_allowed");
    assert!(
        fetcher.requests().is_empty(),
        "the foreign host must not be asked at all"
    );
}

#[tokio::test]
async fn a_redirect_onto_a_foreign_host_is_refused_after_the_hop() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new()
        .redirect(START, 302, "https://evil.test/x")
        .page("https://evil.test/x", "https://a.test/x");
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.target_not_allowed");
    let asked: Vec<String> = fetcher
        .requests()
        .into_iter()
        .map(|request| request.url.to_string())
        .collect();
    assert_eq!(asked, [START]);
}

#[tokio::test]
async fn a_name_that_resolves_into_the_local_network_is_refused_for_v4_and_v6() {
    for address in ["127.0.0.1", "10.4.4.4", "169.254.169.254", "::1", "fd00::1"] {
        let rule = collecting(vec![json!({ "kind": "fetch" })]);
        let fetcher = Recorded::new().page(START, "https://a.test/x");
        let dns = Dns::public().pointing("board.test", &[address]);
        let clock = TestClock::new();
        let refused = Executor::new(&fetcher, &dns, &clock)
            .run(&rule, &url(START))
            .await
            .expect_err("refused");
        assert_eq!(
            refused.code(),
            "site_rules.address_not_public",
            "{address} was allowed"
        );
        assert!(fetcher.requests().is_empty());
    }
}

#[tokio::test]
async fn one_local_address_among_public_ones_is_enough_to_refuse() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new().page(START, "https://a.test/x");
    let dns = Dns::public().pointing("board.test", &["93.184.216.34", "127.0.0.1"]);
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &dns, &clock)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.address_not_public");
}

#[tokio::test]
async fn a_redirect_into_the_local_network_is_refused_although_the_host_is_allowed() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new()
        .redirect(START, 302, "https://intranet.board.test/x")
        .page("https://intranet.board.test/x", "https://a.test/x");
    let dns = Dns::public().pointing("intranet.board.test", &["10.0.0.5"]);
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &dns, &clock)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.address_not_public");
    assert_eq!(fetcher.requests().len(), 1);
}

#[tokio::test]
async fn a_literal_local_address_needs_no_resolver_to_be_refused() {
    let rule: Rule = serde_json::from_value(json!({
        "id": "loopback", "name": "loopback", "group": "board", "version": 1,
        "match": { "hosts": ["board.test"] },
        "steps": [
            { "kind": "fetch", "url": "http://127.0.0.1:8710/api/v1/system" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ],
        "package": { "from": "variable", "name": "links" },
        "probe": "https://board.test/a", "checked": "2026-09-21"
    }))
    .expect("rule");
    let fetcher = Recorded::new();
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    // The host bolt bites first: `127.0.0.1` is not a host this rule's `match` names.
    assert_eq!(refused.code(), "site_rules.target_not_allowed");
    assert!(fetcher.requests().is_empty());
}

#[tokio::test]
async fn a_redirect_loop_ends_as_a_cycle_instead_of_spending_the_budget() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new()
        .redirect(START, 302, "https://board.test/b")
        .redirect("https://board.test/b", 302, START);
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.cycle");
    assert_eq!(fetcher.requests().len(), 2);
}

#[tokio::test]
async fn a_chain_deeper_than_the_limit_is_refused() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new()
        .redirect(START, 302, "https://board.test/2")
        .redirect("https://board.test/2", 302, "https://board.test/3")
        .redirect("https://board.test/3", 302, "https://board.test/4")
        .page("https://board.test/4", "https://a.test/x");
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_limits(Limits {
            max_depth: 2,
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.limit_depth");
    // Two requests were allowed, the third was not issued.
    assert_eq!(fetcher.requests().len(), 2);
}

#[tokio::test]
async fn a_fan_out_wider_than_the_page_limit_is_refused() {
    let rule = rule(
        json!([
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "href=\"(/out/[0-9]+)\"", "into": "raw", "all": true },
            { "kind": "redirect", "from": "raw", "into": "links" }
        ]),
        from_variable("links"),
    );
    let mut page = String::new();
    let mut fetcher = Recorded::new();
    for index in 0..10 {
        page.push_str(&format!("<a href=\"/out/{index}\"></a>"));
        fetcher = fetcher.redirect(
            &format!("https://board.test/out/{index}"),
            302,
            &format!("https://a.test/{index}"),
        );
    }
    let fetcher = fetcher.page(START, &page);
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_limits(Limits {
            max_pages: 3,
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.limit_pages");
    assert_eq!(fetcher.requests().len(), 3);
}

#[tokio::test]
async fn more_links_than_the_limit_allows_is_refused_rather_than_trimmed() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new().page(START, "https://a.test/1 https://a.test/2 https://a.test/3");
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_limits(Limits {
            max_links: 2,
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.limit_links");
}

#[tokio::test]
async fn the_time_budget_ends_a_run_however_many_steps_are_left() {
    let clock = Arc::new(TestClock::new());
    let rule = collecting(vec![
        json!({ "kind": "fetch", "url": "https://board.test/1", "into": "a" }),
        json!({ "kind": "fetch", "url": "https://board.test/2", "into": "b" }),
        json!({ "kind": "fetch", "url": "https://board.test/3", "into": "page" }),
    ]);
    let fetcher = Recorded::new()
        .everything_else(Ok(FetchResponse::ok("https://a.test/x")))
        .charging(&clock, Duration::from_millis(600));
    let refused = Executor::new(&fetcher, &Dns::public(), clock.as_ref())
        .with_limits(Limits {
            max_total_time: Duration::from_secs(1),
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.limit_time");
    // Two requests fit in the budget; the third was never issued.
    assert_eq!(fetcher.requests().len(), 2);
}

#[tokio::test]
async fn a_response_larger_than_the_limit_is_refused_from_either_side() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    // An adapter that ignored the ceiling it was handed.
    let fetcher = Recorded::new().page(START, &"x".repeat(4096));
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_limits(Limits {
            max_response_bytes: 64,
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.response_too_large");
    assert_eq!(
        fetcher.requests().first().map(|request| request.max_bytes),
        Some(64),
        "the ceiling is passed to the adapter as well"
    );
    // An adapter that stopped reading and said so.
    let fetcher = Recorded::new().answer(START, Err(FetchFailure::TooLarge));
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_limits(Limits {
            max_response_bytes: 64,
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert_eq!(refused.code(), "site_rules.response_too_large");
}

#[tokio::test]
async fn every_limit_refusal_says_it_is_a_limit_and_not_a_finding_about_the_service() {
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new().page(START, "https://a.test/1 https://a.test/2");
    let clock = TestClock::new();
    let refused = Executor::new(&fetcher, &Dns::public(), &clock)
        .with_limits(Limits {
            max_links: 1,
            ..Limits::default()
        })
        .run(&rule, &url(START))
        .await
        .expect_err("refused");
    assert!(refused.is_limit());
    assert!(!refused.not_mine());
}

/// The IPv6 forms that carry an IPv4 address, one per form. Each is a local or private IPv4
/// address in IPv6 clothing, and each read as an ordinary global address before RD-110-05's
/// review: `2002:0a00:0001::` *is* `10.0.0.1`.
const EMBEDDED_LOCAL_V6: [&str; 6] = [
    "2002:0a00:0001::",                     // 6to4, RFC 3056 -> 10.0.0.1
    "2002:7f00:0001::1",                    // 6to4 -> 127.0.0.1
    "64:ff9b::7f00:1",                      // NAT64 well-known prefix, RFC 6052 -> 127.0.0.1
    "64:ff9b:1::1",                         // NAT64 network-specific prefix, RFC 8215
    "2606:4700:1:2:0:5efe:a9fe:a9fe",       // ISATAP, RFC 5214 -> 169.254.169.254
    "2001:0:4136:e378:8000:63bf:3fff:fdd2", // Teredo, RFC 4380
];

#[tokio::test]
async fn an_ipv4_address_in_ipv6_clothing_is_refused_as_a_fetch_target() {
    for text in EMBEDDED_LOCAL_V6 {
        let rule = collecting(vec![json!({ "kind": "fetch" })]);
        let fetcher = Recorded::new().page(START, "https://a.test/x");
        let dns = Dns::public().pointing("board.test", &[text]);
        let clock = TestClock::new();
        let refused = Executor::new(&fetcher, &dns, &clock)
            .run(&rule, &url(START))
            .await
            .expect_err("refused");
        assert_eq!(
            refused.code(),
            "site_rules.address_not_public",
            "{text} was allowed as a target"
        );
        assert!(fetcher.requests().is_empty());
    }
}

#[tokio::test]
async fn an_ipv4_address_in_ipv6_clothing_never_becomes_a_link() {
    // The same function guards the links a rule produces, because a rule's output is fetched
    // by the download engine and must not point it at this machine either.
    for text in EMBEDDED_LOCAL_V6 {
        let rule = collecting(vec![json!({ "kind": "fetch" })]);
        let fetcher = Recorded::new().page(START, &format!("https://[{text}]/payload"));
        let clock = TestClock::new();
        let refused = Executor::new(&fetcher, &Dns::public(), &clock)
            .run(&rule, &url(START))
            .await
            .expect_err("refused");
        assert_eq!(
            refused.code(),
            "site_rules.no_links",
            "{text} survived as a link"
        );
    }
}

#[tokio::test]
async fn a_global_address_carrying_a_routable_ipv4_one_is_still_reachable() {
    // Decoding the embedded address must not refuse everything that carries one.
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new().page(START, "https://a.test/x");
    let dns = Dns::public().pointing("board.test", &["2002:5db8:d822::1"]);
    let clock = TestClock::new();
    Executor::new(&fetcher, &dns, &clock)
        .run(&rule, &url(START))
        .await
        .expect("crawled");
}

#[tokio::test]
async fn the_checked_address_is_what_the_adapter_is_told_to_connect_to() {
    // A record with a time-to-live of zero that flips to `127.0.0.1` right after the
    // executor's lookup. The second answer must be inconsequential, because the adapter is
    // handed the first, checked one and is contractually forbidden to resolve again.
    let rule = collecting(vec![json!({ "kind": "fetch" })]);
    let fetcher = Recorded::new().page(START, "https://a.test/x");
    let dns = FlippingDns::default();
    let clock = TestClock::new();
    let crawl = Executor::new(&fetcher, &dns, &clock)
        .run(&rule, &url(START))
        .await
        .expect("crawled");
    assert_eq!(crawl.links, ["https://a.test/x"]);
    assert_eq!(
        dns.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the host is resolved exactly once per request"
    );
    let request = fetcher.requests().into_iter().next().expect("requested");
    assert_eq!(
        request.addresses,
        ["93.184.216.34"
            .parse::<std::net::IpAddr>()
            .expect("address")],
        "the adapter must be handed the address that was checked"
    );
}

#[tokio::test]
async fn a_literal_address_carries_no_pinned_addresses() {
    // Nothing was resolved, so there is nothing to pin; the adapter connects to the address
    // in the URL, which the executor checked directly.
    let rule: Rule = serde_json::from_value(json!({
        "id": "literal", "name": "literal", "group": "board", "version": 1,
        "match": { "hosts": ["93.184.216.34"] },
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "(https://[^ ]+)", "into": "links", "all": true }
        ],
        "package": { "from": "variable", "name": "links" },
        "probe": "https://93.184.216.34/a", "checked": "2026-09-21"
    }))
    .expect("rule");
    let fetcher = Recorded::new().page("https://93.184.216.34/a", "https://a.test/x");
    let clock = TestClock::new();
    Executor::new(&fetcher, &Dns::public(), &clock)
        .run(&rule, &url("https://93.184.216.34/a"))
        .await
        .expect("crawled");
    let request = fetcher.requests().into_iter().next().expect("requested");
    assert!(request.addresses.is_empty());
}

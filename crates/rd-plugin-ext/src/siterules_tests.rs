//! Tests for [`super`]: which rule is asked first, and which refusal keeps the search going.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_siterules::{Catalogue, Crawl, Rule, RunError};
use url::Url;

use super::{RuleOutcome, RuleRunner, SiteRules};

/// A rule that claims every path on `host`, named after its id so the record reads well.
pub(crate) fn rule(id: &str, host: &str) -> Rule {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": id,
        "group": "board",
        "version": 1,
        "match": { "hosts": [host] },
        "steps": [{ "kind": "fetch" }],
        "package": { "from": "title" },
        "probe": format!("https://{host}/a/b"),
        "checked": "2026-09-20"
    }))
    .expect("a valid rule")
}

/// What a stand-in runner answers for one rule id, and the record of which rules ran.
pub(crate) struct FakeRunner {
    pub(crate) answers: Vec<(String, Result<Crawl, RunError>)>,
    pub(crate) asked: Arc<Mutex<Vec<String>>>,
}

impl FakeRunner {
    pub(crate) fn new(
        answers: Vec<(&str, Result<Crawl, RunError>)>,
        asked: &Arc<Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            answers: answers
                .into_iter()
                .map(|(id, answer)| (id.to_owned(), answer))
                .collect(),
            asked: Arc::clone(asked),
        }
    }
}

#[async_trait]
impl RuleRunner for FakeRunner {
    async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        self.asked.lock().expect("asked").push(rule.id.clone());
        self.answers
            .iter()
            .find(|(id, _)| id == &rule.id)
            .map(|(_, answer)| answer.clone())
            // What the executor itself answers for an address a rule does not claim.
            .unwrap_or_else(|| Err(RunError::NotClaimed(address.to_string())))
    }
}

pub(crate) fn crawl(links: &[&str], package: Option<&str>) -> Crawl {
    crawl_with_mirrors(links, package, false)
}

/// The same, for a rule that says its page is one release (RD-110-18).
pub(crate) fn crawl_with_mirrors(links: &[&str], package: Option<&str>, mirrors: bool) -> Crawl {
    Crawl {
        address: "https://board.example.org/a/b".parse().expect("url"),
        links: links.iter().map(|link| (*link).to_owned()).collect(),
        package_name: package.map(str::to_owned),
        pages_fetched: 1,
        mirrors,
    }
}

fn address() -> Url {
    "https://board.example.org/a/b".parse().expect("url")
}

/// A rule the person wrote is asked before the shipped one that claims the same address.
/// Without that a broken shipped rule could not be bridged without waiting for a release.
#[tokio::test]
async fn a_rule_of_ones_own_is_asked_before_the_shipped_one() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut catalogue = Catalogue::new(vec![rule("shipped", "board.example.org")]);
    catalogue
        .add_user_rule(rule("mine", "board.example.org"))
        .expect("admitted");
    let runner = FakeRunner::new(
        vec![
            ("mine", Ok(crawl(&["https://host.example.org/a.bin"], None))),
            (
                "shipped",
                Ok(crawl(&["https://host.example.org/b.bin"], None)),
            ),
        ],
        &asked,
    );
    let rules = SiteRules::new(catalogue, Arc::new(runner));

    let outcome = rules.consult(&address()).await.expect("a rule spoke");
    assert!(
        matches!(&outcome, RuleOutcome::Crawled { crawl, .. }
            if crawl.links == ["https://host.example.org/a.bin"]),
        "{outcome:?}"
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["mine".to_owned()],
        "the shipped rule was never even asked"
    );
}

/// "This was never my page" is the one refusal that says nothing about the page, so the next
/// rule gets its turn — and when none of them claims the address, the selection moves on.
#[tokio::test]
async fn an_unclaimed_address_goes_to_the_next_rule_and_then_past_the_rules() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let catalogue = Catalogue::new(vec![
        rule("first", "board.example.org"),
        rule("second", "board.example.org"),
    ]);
    let runner = FakeRunner::new(
        vec![
            ("first", Err(RunError::NotClaimed("x".to_owned()))),
            (
                "second",
                Ok(crawl(&["https://host.example.org/a.bin"], None)),
            ),
        ],
        &asked,
    );
    let rules = SiteRules::new(catalogue, Arc::new(runner));
    assert!(matches!(
        rules.consult(&address()).await,
        Some(RuleOutcome::Crawled { .. })
    ));
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["first".to_owned(), "second".to_owned()]
    );

    let asked = Arc::new(Mutex::new(Vec::new()));
    let rules = SiteRules::new(
        Catalogue::new(vec![rule("first", "board.example.org")]),
        Arc::new(FakeRunner::new(Vec::new(), &asked)),
    );
    assert_eq!(rules.consult(&address()).await, None, "nobody spoke");
}

/// Every other code is a statement about *this* page: it ends the search rather than being
/// handed to the next rule, which would only produce a second statement about the same page.
#[tokio::test]
async fn a_dead_page_ends_the_search_at_the_rule_that_found_it() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let catalogue = Catalogue::new(vec![
        rule("first", "board.example.org"),
        rule("second", "board.example.org"),
    ]);
    let runner = FakeRunner::new(
        vec![(
            "first",
            Err(RunError::PageDead {
                url: "https://board.example.org/a/b".to_owned(),
                reason: "404".to_owned(),
            }),
        )],
        &asked,
    );
    let rules = SiteRules::new(catalogue, Arc::new(runner));

    let outcome = rules.consult(&address()).await.expect("a rule spoke");
    assert!(
        matches!(&outcome, RuleOutcome::Refused { error, .. }
            if error.code() == "site_rules.page_dead"),
        "{outcome:?}"
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["first".to_owned()],
        "the second rule was not asked about a page that is gone"
    );
}

/// The rules in force can be replaced, so a rule written in the interface (RD-110-08) takes
/// effect without a restart.
#[tokio::test]
async fn the_rules_in_force_can_be_replaced() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rules = SiteRules::new(
        Catalogue::default(),
        Arc::new(FakeRunner::new(
            vec![("mine", Ok(crawl(&["https://host.example.org/a.bin"], None)))],
            &asked,
        )),
    );
    assert!(rules.is_empty());
    assert_eq!(rules.consult(&address()).await, None);

    let mut catalogue = Catalogue::default();
    catalogue
        .add_user_rule(rule("mine", "board.example.org"))
        .expect("admitted");
    rules.replace(catalogue);
    assert!(!rules.is_empty());
    assert!(matches!(
        rules.consult(&address()).await,
        Some(RuleOutcome::Crawled { .. })
    ));
}

/// A rule the self-test found dead is skipped, not deleted: it costs no request, it stays in
/// the catalogue, and the next rule in line gets the address (RD-110-09).
#[tokio::test]
async fn a_rule_the_self_test_found_dead_is_not_asked_again() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut catalogue = Catalogue::new(vec![rule("shipped", "board.example.org")]);
    catalogue
        .add_user_rule(rule("mine", "board.example.org"))
        .expect("admitted");
    let runner = FakeRunner::new(
        vec![
            (
                "mine",
                Err(RunError::PageDead {
                    url: "https://board.example.org/a/b".to_owned(),
                    reason: "no such host".to_owned(),
                }),
            ),
            (
                "shipped",
                Ok(crawl(&["https://host.example.org/b.bin"], None)),
            ),
        ],
        &asked,
    );
    let rules = SiteRules::new(catalogue, Arc::new(runner));
    rules.set_dead(["mine".to_owned()].into_iter().collect());

    let outcome = rules.consult(&address()).await.expect("a rule spoke");
    assert!(
        matches!(&outcome, RuleOutcome::Crawled { rule, .. } if rule == "shipped"),
        "{outcome:?}"
    );
    assert_eq!(
        asked.lock().expect("asked").as_slice(),
        ["shipped"],
        "the dead rule was asked anyway"
    );
    assert_eq!(rules.dead().len(), 1);
    assert!(
        rules.catalogue().get("mine").is_some(),
        "a dead rule is skipped, not deleted"
    );

    // A later run that finds the service alive again brings it back into the order.
    rules.set_dead(std::collections::BTreeSet::new());
    let outcome = rules.consult(&address()).await.expect("a rule spoke");
    assert!(
        matches!(&outcome, RuleOutcome::Refused { rule, error }
            if rule == "mine" && error.code() == "site_rules.page_dead"),
        "{outcome:?}"
    );
}

#[test]
fn asking_only_whether_a_rule_claims_an_address_costs_no_request() {
    // RD-110-21. A watched listing asks this of every anchor on the page, so it must be the
    // cheap half of `consult` -- and it must agree with it, dead rules included, or a
    // subscription would collect items the crawl selection then declines to read.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut catalogue = Catalogue::new(vec![rule("shipped", "board.example.org")]);
    catalogue
        .add_user_rule(rule("mine", "gone.example.org"))
        .expect("admitted");
    let rules = SiteRules::new(catalogue, Arc::new(FakeRunner::new(Vec::new(), &asked)));

    assert!(rules.claims(&"https://board.example.org/a/b".parse().expect("url")));
    assert!(rules.claims(&"https://gone.example.org/a/b".parse().expect("url")));
    assert!(!rules.claims(&"https://elsewhere.example.org/a/b".parse().expect("url")));
    assert!(
        asked.lock().expect("asked").is_empty(),
        "no rule may be run to answer this"
    );

    rules.set_dead(["mine".to_owned()].into_iter().collect());
    assert!(!rules.claims(&"https://gone.example.org/a/b".parse().expect("url")));
}

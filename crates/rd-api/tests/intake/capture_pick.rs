//! RD-1190-17: a series page that reaches the intake through the capture surface -- a copied
//! link, the browser extension, Click'n'Load -- lands on the pick board exactly as a paste in
//! the LinkGrabber does, and the interface hears of it on the event bus. The same page captured
//! again keeps its list: the capture agent repeated such a capture, and every repeat used to
//! replace the drawer's list under a new id.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::http::StatusCode;
use rd_siterules::{
    Catalogue, Crawl, CrawlGroup, PickEntry, PickList, Rule, RunError, exec::Variables,
};
use url::Url;

use crate::common::{self, post_capture};

const PAGE: &str = "https://series.example/serie/show/";

/// A two-stage rule for the page, as serienjunkies.org's is.
fn rule() -> Rule {
    serde_json::from_value(serde_json::json!({
        "id": "series",
        "name": "series.example",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["series.example"] },
        "steps": [{ "kind": "fetch" }, { "kind": "regex", "pattern": "(r\\d)", "into": "releases",
                    "all": true }],
        "package": { "from": "title" },
        "groups": {
            "from": "releases",
            "pick": { "attributes": { "season": "(\\d)" } },
            "steps": [{ "kind": "regex", "from": "entry", "pattern": "(.+)", "into": "links" }],
            "package": { "from": "variable", "name": "entry" }
        },
        "probe": PAGE,
        "checked": "2026-10-08"
    }))
    .expect("rule")
}

/// Lists two releases and is never asked to resolve one here.
struct Listing;

#[async_trait]
impl rd_plugin_ext::RuleRunner for Listing {
    async fn run(&self, _rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        let entry = |text: &str| PickEntry {
            text: text.to_owned(),
            label: Some(format!("Show.{text}")),
            attributes: BTreeMap::from([("season".to_owned(), "1".to_owned())]),
        };
        Ok(Crawl {
            address: address.clone(),
            links: Vec::new(),
            package_name: Some("Show".to_owned()),
            pages_fetched: 1,
            mirrors: false,
            groups: Vec::new(),
            pick: Some(PickList {
                entries: vec![entry("r1"), entry("r2")],
                variables: Variables::default(),
            }),
        })
    }

    async fn resolve(
        &self,
        _rule: &Rule,
        _address: &Url,
        _list: &PickList,
        index: usize,
    ) -> Result<CrawlGroup, RunError> {
        Err(RunError::NoEntry(index))
    }
}

#[tokio::test]
async fn a_captured_series_page_is_announced_and_keeps_its_list_when_captured_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let rules = Arc::new(rd_plugin_ext::SiteRules::new(
        Catalogue::new(vec![rule()]),
        Arc::new(Listing),
    ));
    let crawlers = Arc::new(rd_plugin_ext::FolderCrawlers::none().with_rules(Arc::clone(&rules)));
    let router = rd_api::router(harness.state.clone().with_crawlers(crawlers));
    let mut events = harness.database.subscribe();
    let copied = serde_json::json!({
        "text": PAGE,
        "source": "clipboard",
        "source_label": "Capture-Agent"
    });

    // The answer the capture clients now take as a success: the list waits for a choice.
    let (status, refused) = post_capture(&router, copied.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "site_rules.pick_waiting");
    assert_eq!(refused["params"]["entries"], "2");
    let list = refused["params"]["list"]
        .as_str()
        .expect("the list's id")
        .to_owned();

    // The interface hears of it, though the paste was not its own.
    let announced = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("the bus stays open");
            if event.kind == rd_core::EventKind::CollectorChanged
                && let Some(listed) = event.payload.get("pick_listed")
            {
                return listed.clone();
            }
        }
    })
    .await
    .expect("the listing is announced");
    assert_eq!(announced["list"], list.as_str());
    assert_eq!(announced["entries"], 2);
    assert_eq!(announced["rule"], "series.example");

    // Captured again, as the agent used to repeat it: the same list, under the same id.
    let (status, again) = post_capture(&router, copied).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{again}");
    assert_eq!(again["params"]["list"], list.as_str());
    let ids: Vec<String> = rules
        .picks()
        .pages()
        .into_iter()
        .map(|page| page.id)
        .collect();
    assert_eq!(ids, std::slice::from_ref(&list));

    // A list that is gone says why, so the interface can close quietly or list it again.
    assert!(rules.picks().remove(&list));
    let (status, body) =
        common::get_json(&router, &format!("/api/v1/collector/picks/{list}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "site_rules.pick_not_found");
    assert_eq!(body["params"]["reason"], "discarded");
}

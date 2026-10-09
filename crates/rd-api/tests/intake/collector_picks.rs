//! The pick board over REST (RD-1170-03, audit 2026-10-08 API-04): the six
//! `/api/v1/collector/picks` routes, which only the MCP tools drove until now.
//!
//! What is asserted is the contract a client builds on: the address check and its code, the
//! refusal of an address no rule claims and of an entry the list does not have, and the shape
//! of a list as it is read, resolved, stopped and discarded. How long a list lives on the board
//! is the board's own business (`crates/rd-plugin-ext/src/picks.rs`) and is not pinned here.
//!
//! The network is the synthetic series site's answer behind the runner seam, as in the MCP
//! suite (`tests/mcp/mcp/series_pick.rs`); the captcha broker never answers, so a started round
//! stays where the test can stop it.

use std::{net::IpAddr, sync::Arc};

use async_trait::async_trait;
use axum::http::StatusCode;
use rd_siterules::{
    CaptchaRequest, CaptchaSolver, Crawl, CrawlGroup, Executor, FetchFailure, FetchRequest,
    FetchResponse, Fetcher, HostResolver, PickList, Rule, RunError, SystemClock,
};
use serde_json::{Value, json};
use url::Url;

use crate::common::{self, delete_json, get_json, post_json};

const RULE: &str = include_str!("../../../rd-siterules/tests/fixtures/series-rule.json");
const PAGE: &str = include_str!("../../../rd-siterules/tests/fixtures/series-page.html");
const RELEASES: &str = include_str!("../../../rd-siterules/tests/fixtures/series-releases.json");
const PROBE: &str = "https://series.example.com/serie/example-open-series/";
const API: &str = "https://series.example.com/api/media/6a96aa7cff33487fe64137e1/releases";
const PICKS: &str = "/api/v1/collector/picks";

/// The series page and its release list, and nothing else.
struct RecordedSite;

#[async_trait]
impl Fetcher for RecordedSite {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure> {
        match request.url.as_str() {
            PROBE => Ok(FetchResponse::ok(PAGE)),
            API => Ok(FetchResponse::ok(RELEASES)),
            other => Err(FetchFailure::Unreachable(format!(
                "{other} was not recorded"
            ))),
        }
    }
}

#[async_trait]
impl HostResolver for RecordedSite {
    async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
        Ok(vec![IpAddr::from([93, 184, 216, 34])])
    }
}

/// A person who never gets round to the captcha.
struct NobodySolves;

#[async_trait]
impl CaptchaSolver for NobodySolves {
    async fn solve(&self, _request: CaptchaRequest) -> Result<String, String> {
        std::future::pending().await
    }
}

/// The real executor over the recorded site.
struct RecordedRunner;

#[async_trait]
impl rd_plugin_ext::RuleRunner for RecordedRunner {
    async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        let clock = SystemClock::new();
        Executor::new(&RecordedSite, &RecordedSite, &clock)
            .with_captcha(&NobodySolves)
            .with_device_id("0123456789abcdef0123456789abcdef")
            .run(rule, address)
            .await
    }

    async fn resolve(
        &self,
        rule: &Rule,
        address: &Url,
        list: &PickList,
        index: usize,
    ) -> Result<CrawlGroup, RunError> {
        let clock = SystemClock::new();
        Executor::new(&RecordedSite, &RecordedSite, &clock)
            .with_captcha(&NobodySolves)
            .with_device_id("0123456789abcdef0123456789abcdef")
            .resolve(rule, address, list, index)
            .await
    }
}

/// A router whose site rules run over the recorded site, with the synthetic series rule saved
/// and switched on through the REST route a person's editor uses.
async fn router_with_the_series_rule(directory: &std::path::Path) -> axum::Router {
    let harness = common::test_harness(directory).await;
    let rules = Arc::new(rd_plugin_ext::SiteRules::new(
        rd_siterules::Catalogue::default(),
        Arc::new(RecordedRunner),
    ));
    let crawlers = Arc::new(rd_plugin_ext::FolderCrawlers::none().with_rules(rules));
    let router = rd_api::router(harness.state.clone().with_crawlers(crawlers));

    let rule: Value = serde_json::from_str(RULE).expect("the synthetic series rule");
    let (status, saved) = post_json(
        &router,
        "/api/v1/site-rules",
        json!({ "rule": rule, "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    router
}

/// Every field a client reads off one list, with the types it reads them as.
fn assert_list_shape(page: &Value) {
    assert!(
        page["id"].as_str().is_some_and(|id| !id.is_empty()),
        "{page}"
    );
    assert_eq!(page["rule_id"], "example-series", "{page}");
    assert!(page["rule"].is_string(), "{page}");
    assert_eq!(page["address"], PROBE, "{page}");
    assert!(page["created_at"].is_string(), "{page}");
    assert!(page["running"].is_boolean(), "{page}");
    assert!(
        page["total"].is_u64() && page["finished"].is_u64(),
        "{page}"
    );
    assert!(page["waiting_for_captcha"].is_boolean(), "{page}");
    let entries = page["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 32, "{page}");
    for (position, entry) in entries.iter().enumerate() {
        assert_eq!(entry["index"], position, "{entry}");
        assert!(entry["attributes"].is_object(), "{entry}");
        assert!(entry["state"].is_string(), "{entry}");
        assert!(entry["links"].is_u64(), "{entry}");
    }
}

/// An address the board cannot list is refused before any rule is consulted, with the code
/// and the address as a parameter; an installation without rules lists an empty board.
#[tokio::test]
async fn an_address_that_is_not_http_is_refused_with_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;

    for address in [
        "not an address",
        "ftp://example.org/release/1",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://",
        "",
    ] {
        let (status, body) = post_json(&harness.router, PICKS, json!({ "address": address })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{address:?}: {body}");
        assert_eq!(body["code"], "site_rules.invalid_address", "{address:?}");
        // Redacted like every parameter, so only its presence is the contract.
        assert!(body["params"]["address"].is_string(), "{address:?}: {body}");
    }

    let (status, body) = post_json(&harness.router, PICKS, json!({})).await;
    assert!(
        status.is_client_error(),
        "an address is required: {status} {body}"
    );

    let (status, board) = get_json(&harness.router, PICKS).await;
    assert_eq!(status, StatusCode::OK, "{board}");
    assert_eq!(board["pages"], json!([]), "{board}");
}

/// An address no switched-on rule claims is refused with its own code, not listed.
#[tokio::test]
async fn an_address_no_rule_claims_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = router_with_the_series_rule(directory.path()).await;

    let (status, body) = post_json(
        &router,
        PICKS,
        json!({ "address": "https://example.org/release/1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "site_rules.not_claimed", "{body}");

    let (_, board) = get_json(&router, PICKS).await;
    assert_eq!(
        board["pages"],
        json!([]),
        "a refusal lists nothing: {board}"
    );
}

/// One list through every route: listed, read alone and on the board, an unknown entry
/// refused, an entry resolved, the round stopped and the list discarded.
#[tokio::test]
async fn a_listed_page_is_read_resolved_stopped_and_discarded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = router_with_the_series_rule(directory.path()).await;

    let (status, listed) = post_json(&router, PICKS, json!({ "address": PROBE })).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_list_shape(&listed);
    assert_eq!(listed["running"], false, "{listed}");
    assert_eq!(
        (listed["total"].as_u64(), listed["finished"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(listed["waiting_for_captcha"], false, "{listed}");
    let entries = listed["entries"].as_array().expect("entries");
    assert!(
        entries
            .iter()
            .all(|entry| entry["state"] == "pending" && entry["code"].is_null()),
        "a fresh list waits for a choice: {listed}"
    );
    let id = listed["id"].as_str().expect("id").to_owned();
    let one = format!("{PICKS}/{id}");

    let (status, board) = get_json(&router, PICKS).await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let pages = board["pages"].as_array().expect("pages");
    assert_eq!(pages.len(), 1, "{board}");
    assert_eq!(pages[0]["id"], id.as_str());
    assert_list_shape(&pages[0]);

    let (status, read) = get_json(&router, &one).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_list_shape(&read);
    assert_eq!(read["id"], id.as_str());

    // An index past the end is refused by name, and nothing is queued.
    let (status, refused) = post_json(
        &router,
        &format!("{one}/resolve"),
        json!({ "entries": [3, 32] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "site_rules.no_entry", "{refused}");
    assert_eq!(refused["params"]["entry"], "32", "{refused}");
    let (_, unchanged) = get_json(&router, &one).await;
    assert_eq!(unchanged["running"], false, "{unchanged}");
    assert_eq!(unchanged["entries"][3]["state"], "pending", "{unchanged}");

    // Resolving answers at once with the queue it started.
    let (status, started) = post_json(
        &router,
        &format!("{one}/resolve"),
        json!({ "entries": [3] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_list_shape(&started);
    assert_eq!(started["running"], true, "{started}");
    assert_eq!(started["total"], 1, "{started}");
    assert_eq!(started["entries"][3]["state"], "queued", "{started}");

    // Stopping puts the chosen entry back, saying why.
    let (status, stopped) = post_json(&router, &format!("{one}/cancel"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_list_shape(&stopped);
    assert_eq!(stopped["running"], false, "{stopped}");
    assert_eq!(stopped["waiting_for_captcha"], false, "{stopped}");
    assert_eq!(stopped["entries"][3]["state"], "pending", "{stopped}");
    assert_eq!(
        stopped["entries"][3]["code"], "site_rules.pick_cancelled",
        "{stopped}"
    );

    let (status, discarded) = delete_json(&router, &one).await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    assert_eq!(
        discarded["code"], "site_rules.pick_discarded",
        "{discarded}"
    );
    let (_, board) = get_json(&router, PICKS).await;
    assert_eq!(
        board["pages"],
        json!([]),
        "the board is empty again: {board}"
    );
}

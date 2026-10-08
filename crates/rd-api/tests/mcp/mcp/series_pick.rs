//! RD-1170-03: an agent writes the two-stage serienjunkies.org rule, tries it, saves it, lists
//! a series page's releases, picks one and starts resolving it -- with the MCP tools alone. The
//! captcha stays a person's: the tool reports it waiting, the (fake) broker answers it, and the
//! release lands in the LinkGrabber as one package named after it.
//!
//! The network is the recorded answer of serienjunkies.org (2026-10-07) behind the runner seam
//! the service uses; the links answer of the chosen release is the assumed shape the rule's
//! last steps read (see `crates/rd-siterules/tests/series_pick.rs`).

use std::{net::IpAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use rd_siterules::{
    CaptchaRequest, CaptchaSolver, Crawl, CrawlGroup, Executor, FetchFailure, FetchRequest,
    FetchResponse, Fetcher, HostResolver, PickList, Rule, RunError, SystemClock,
};
use tokio::sync::Semaphore;
use url::Url;

use super::{API_BEARER, everything::ok, everything::refused_with, handshake};
use crate::common::{self, Options};

const PAYLOAD: &str = include_str!("../../../../rd-siterules/resources/site-rules-payload.json");
const PAGE: &str = include_str!("../../../../rd-siterules/tests/fixtures/serienjunkies-page.html");
const RELEASES: &str =
    include_str!("../../../../rd-siterules/tests/fixtures/serienjunkies-releases.json");
const DOWNLOADS: &str =
    include_str!("../../../../rd-siterules/tests/fixtures/serienjunkies-downloads.json");
const PROBE: &str = "https://serienjunkies.org/serie/the-varnell-hill-show/";
const API: &str = "https://serienjunkies.org/api/media/6a96aa7cff33487fe64137e1/releases";
const CHOSEN: &str =
    "https://serienjunkies.org/api/releases/6ac64b43b35595d9966c903e/downloads/ddownload";
const RELEASE: &str = "The.Varnell.Hill.Show.2026.S01E07.German.DL.720p.WEB.h264-WvF";

/// serienjunkies.org's recorded answers, and nothing else.
struct RecordedSite;

#[async_trait]
impl Fetcher for RecordedSite {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure> {
        match request.url.as_str() {
            PROBE => Ok(FetchResponse::ok(PAGE)),
            API => Ok(FetchResponse::ok(RELEASES)),
            CHOSEN if request.json && request.form.contains_key("recaptchaToken") => {
                Ok(FetchResponse::ok(DOWNLOADS))
            }
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

/// The captcha broker with a person in front of it: answers once the test lets the person
/// solve, and counts what it was asked.
struct FakeBroker {
    solved: Semaphore,
    asked: std::sync::Mutex<Vec<CaptchaRequest>>,
}

#[async_trait]
impl CaptchaSolver for FakeBroker {
    async fn solve(&self, request: CaptchaRequest) -> Result<String, String> {
        self.asked.lock().expect("asked").push(request);
        let permit = self
            .solved
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        permit.forget();
        Ok("solved-by-a-person".to_owned())
    }
}

/// The real executor over the recorded site and the fake broker.
struct RecordedRunner(Arc<FakeBroker>);

#[async_trait]
impl rd_plugin_ext::RuleRunner for RecordedRunner {
    async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        let clock = SystemClock::new();
        Executor::new(&RecordedSite, &RecordedSite, &clock)
            .with_captcha(self.0.as_ref())
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
            .with_captcha(self.0.as_ref())
            .with_device_id("0123456789abcdef0123456789abcdef")
            .resolve(rule, address, list, index)
            .await
    }
}

/// The serienjunkies.org rule document as the release file will carry it.
fn serienjunkies_rule() -> serde_json::Value {
    let payload: serde_json::Value = serde_json::from_str(PAYLOAD).expect("the payload");
    payload["rules"]
        .as_array()
        .and_then(|rules| rules.iter().find(|rule| rule["id"] == "serienjunkies"))
        .cloned()
        .expect("the payload carries serienjunkies")
}

/// Polls `get_page_pick` until `done` holds, or fails after five seconds.
async fn wait_for(
    router: &axum::Router,
    session: &str,
    id: &str,
    done: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    for _ in 0..500 {
        let page = ok(
            router,
            session,
            "get_page_pick",
            serde_json::json!({ "id": id }),
        )
        .await;
        if done(&page) {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the page never reached the awaited state");
}

#[tokio::test]
async fn an_agent_writes_the_two_stage_rule_lists_a_series_page_and_resolves_one_release() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(directory.path(), Options::default().login()).await;
    let broker = Arc::new(FakeBroker {
        solved: Semaphore::new(0),
        asked: std::sync::Mutex::new(Vec::new()),
    });
    let rules = Arc::new(rd_plugin_ext::SiteRules::new(
        rd_siterules::Catalogue::default(),
        Arc::new(RecordedRunner(Arc::clone(&broker))),
    ));
    let crawlers = Arc::new(rd_plugin_ext::FolderCrawlers::none().with_rules(Arc::clone(&rules)));
    let router = rd_api::router(harness.state.clone().with_crawlers(crawlers));
    let session = handshake(&router, API_BEARER).await;
    let rule = serienjunkies_rule();

    // Tried first: the trial lists the entries and asks no captcha.
    let tried = ok(
        &router,
        &session,
        "test_site_rule",
        serde_json::json!({ "rule": rule, "address": PROBE }),
    )
    .await;
    assert!(tried["error"].is_null(), "{tried}");
    assert_eq!(
        tried["entries"].as_array().map(Vec::len),
        Some(32),
        "{tried}"
    );
    assert_eq!(tried["links"].as_array().map(Vec::len), Some(0));
    assert!(broker.asked.lock().expect("asked").is_empty());

    ok(
        &router,
        &session,
        "create_site_rule",
        serde_json::json!({ "rule": rule, "enabled": true }),
    )
    .await;

    // A paste of the page resolves nothing: it lands on the board and says so.
    let code = refused_with(
        &router,
        &session,
        "collect_links",
        serde_json::json!({ "text": PROBE }),
    )
    .await;
    assert_eq!(code, "site_rules.pick_waiting");

    // The agent lists the page and chooses season 1, episode 7, in 720p.
    let listed = ok(
        &router,
        &session,
        "list_page_entries",
        serde_json::json!({ "address": PROBE }),
    )
    .await;
    let id = listed["id"].as_str().expect("the list's id").to_owned();
    let entries = listed["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 32);
    let chosen = entries
        .iter()
        .find(|entry| {
            entry["attributes"]["season"] == "1"
                && entry["attributes"]["episode"] == "7"
                && entry["attributes"]["resolution"] == "720p"
        })
        .expect("S01E07 in 720p is listed");
    assert_eq!(chosen["label"], RELEASE);
    assert_eq!(chosen["state"], "pending");
    let index = chosen["index"].as_u64().expect("index");
    let boards = ok(&router, &session, "list_page_picks", serde_json::json!({})).await;
    assert_eq!(
        boards["pages"].as_array().map(Vec::len),
        Some(1),
        "the paste and the listing are one list: {boards}"
    );

    // Resolving answers at once; the entry then waits for a person.
    let started = ok(
        &router,
        &session,
        "resolve_page_entries",
        serde_json::json!({ "id": id, "entries": [index] }),
    )
    .await;
    assert_eq!(started["running"], true);
    assert_eq!(started["total"], 1);
    let waiting = wait_for(&router, &session, &id, |page| {
        page["waiting_for_captcha"] == true && !broker.asked.lock().expect("asked").is_empty()
    })
    .await;
    assert_eq!(waiting["entries"][index as usize]["state"], "captcha");
    {
        let asked = broker.asked.lock().expect("asked");
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].page_url.as_str(), PROBE);
    }

    // The person solves it; the release becomes one package in the LinkGrabber.
    broker.solved.add_permits(1);
    let finished = wait_for(&router, &session, &id, |page| page["running"] == false).await;
    let entry = &finished["entries"][index as usize];
    assert_eq!(entry["state"], "done", "{finished}");
    assert_eq!(entry["links"], 2);
    assert_eq!(
        (finished["finished"].as_u64(), finished["total"].as_u64()),
        (Some(1), Some(1))
    );
    let packages = ok(&router, &session, "list_collector", serde_json::json!({})).await;
    let text = packages.to_string();
    assert!(
        text.contains(&format!("\"{RELEASE}\"")),
        "one package named after the release: {text}"
    );
    assert!(
        text.contains("https://ddownload.com/sj1a2b3c4d5e"),
        "{text}"
    );

    // Stopping a finished page changes nothing; discarding it removes it.
    ok(
        &router,
        &session,
        "cancel_page_pick",
        serde_json::json!({ "id": id }),
    )
    .await;
    ok(
        &router,
        &session,
        "discard_page_pick",
        serde_json::json!({ "id": id }),
    )
    .await;
    let code = refused_with(
        &router,
        &session,
        "get_page_pick",
        serde_json::json!({ "id": id }),
    )
    .await;
    assert_eq!(code, "site_rules.pick_not_found");
}

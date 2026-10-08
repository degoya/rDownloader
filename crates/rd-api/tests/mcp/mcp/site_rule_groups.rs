//! RD-1170-02: an agent writes a rule for a page with several releases -- tries it, saves it,
//! finds it -- with the MCP tools alone, and the saved rule turns the page into one package
//! per release with its hosters as mirrors.
//!
//! The network is the recorded answer of warez.cx (2026-10-07) behind the same runner seam
//! the service uses: the trial run asks the runner of the installed rule selection, so this
//! installation hands in one that runs the real executor over a recorded fetcher. The two
//! hosters are registered as providers, the way their plugins register them, so the trial
//! run's verdict on each link is "claimed" without a probe leaving the machine.

use std::{collections::HashMap, net::IpAddr, sync::Arc};

use async_trait::async_trait;
use rd_provider_registry::{
    CredentialKind, DynamicProvider, ProviderKind, ProviderSource, ProviderSpec, TransferAuth,
    replace_dynamic,
};
use rd_siterules::{
    Crawl, Executor, FetchFailure, FetchRequest, FetchResponse, Fetcher, HostResolver, Rule,
    RunError, SystemClock,
};
use url::Url;

use super::{API_BEARER, everything::ok, handshake};
use crate::common::{self, Options};

const PAYLOAD: &str = include_str!("../../../../rd-siterules/resources/site-rules-payload.json");
const WAREZ_API: &str = include_str!("../../../../rd-siterules/tests/fixtures/warez-cx-api.json");
const WAREZ_API_URL: &str = "https://api.warez.cx/start/d/9IMDqgvdVQQ6";
const PROBE: &str = "https://warez.cx/detail/9IMDqgvdVQQ6/The-Beginning-After-the-End";

/// The network a rule sees here: warez.cx's recorded answer, and nothing else.
struct RecordedNetwork;

#[async_trait]
impl Fetcher for RecordedNetwork {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure> {
        if request.url.as_str() == WAREZ_API_URL {
            Ok(FetchResponse::ok(WAREZ_API))
        } else {
            Err(FetchFailure::Unreachable(format!(
                "{} was not recorded",
                request.url
            )))
        }
    }
}

#[async_trait]
impl HostResolver for RecordedNetwork {
    async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
        Ok(vec![IpAddr::from([93, 184, 216, 34])])
    }
}

/// The real executor over the recorded network.
struct RecordedRunner;

#[async_trait]
impl rd_plugin_ext::RuleRunner for RecordedRunner {
    async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        let clock = SystemClock::new();
        Executor::new(&RecordedNetwork, &RecordedNetwork, &clock)
            .run(rule, address)
            .await
    }

    async fn resolve(
        &self,
        rule: &Rule,
        address: &Url,
        list: &rd_siterules::PickList,
        index: usize,
    ) -> Result<rd_siterules::CrawlGroup, RunError> {
        let clock = SystemClock::new();
        Executor::new(&RecordedNetwork, &RecordedNetwork, &clock)
            .resolve(rule, address, list, index)
            .await
    }
}

/// A hoster as its plugin registers it: the host is known, so no link of it is probed.
fn hoster(slug: &str, host: &str) -> DynamicProvider {
    DynamicProvider {
        plugin_id: format!("plugin-{slug}"),
        spec: ProviderSpec {
            slug: slug.to_owned(),
            display_name: host.to_owned(),
            kind: ProviderKind::Hoster,
            credentials: CredentialKind::ApiKey,
            username_required: false,
            transfer_auth: TransferAuth::None,
            secrets: Vec::new(),
            request_domains: vec![host.to_owned()],
            cookie_scope: None,
            match_hosts: vec![host.to_owned()],
            host_aliases: Vec::new(),
            source: ProviderSource::Plugin,
            plugin_id: Some(format!("plugin-{slug}")),
            plugin_version: Some("1.0.0".to_owned()),
        },
    }
}

/// The warez.cx rule document as the release file will carry it.
fn warez_rule() -> serde_json::Value {
    let payload: serde_json::Value = serde_json::from_str(PAYLOAD).expect("the payload");
    payload["rules"]
        .as_array()
        .and_then(|rules| rules.iter().find(|rule| rule["id"] == "warez-cx"))
        .cloned()
        .expect("the payload carries warez-cx")
}

#[tokio::test]
async fn an_agent_writes_tests_and_saves_a_rule_with_a_package_per_release() {
    let rejected = replace_dynamic(vec![
        hoster("ddownload", "ddownload.com"),
        hoster("rapidgator", "rapidgator.net"),
    ]);
    assert!(rejected.is_empty(), "{rejected:?}");
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(directory.path(), Options::default().login()).await;
    let rules = Arc::new(rd_plugin_ext::SiteRules::new(
        rd_siterules::Catalogue::default(),
        Arc::new(RecordedRunner),
    ));
    let crawlers = Arc::new(rd_plugin_ext::FolderCrawlers::none().with_rules(Arc::clone(&rules)));
    let router = rd_api::router(harness.state.clone().with_crawlers(Arc::clone(&crawlers)));
    let session = handshake(&router, API_BEARER).await;
    let rule = warez_rule();

    // Tried first, as the tool description says: four packages, the hosters as mirrors.
    let tried = ok(
        &router,
        &session,
        "test_site_rule",
        serde_json::json!({ "rule": rule, "address": PROBE }),
    )
    .await;
    assert!(tried["error"].is_null(), "{tried}");
    assert_eq!(tried["links"].as_array().map(Vec::len), Some(54), "{tried}");
    assert_eq!(tried["kept"], 54, "every link is a known hoster's: {tried}");
    let groups = tried["groups"].as_array().expect("groups");
    let names: Vec<&str> = groups
        .iter()
        .map(|group| group["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        [
            "The.Beginning.After.the.End.2025.S01.German.Subbed.ANiME.720p.AMZN.WEB.H264-WAREZCX",
            "The.Beginning.After.the.End.2025.S01.German.Subbed.ANiME.1080p.AMZN.WEB.H264-WAREZCX",
            "The.Beginning.After.the.End.2025.S02.German.Subbed.ANiME.720p.AMZN.WEB.H264-WAREZCX",
            "The.Beginning.After.the.End.2025.S02.German.Subbed.ANiME.1080p.AMZN.WEB.H264-WAREZCX",
        ]
    );
    let first = groups[0]["links"].as_array().expect("links");
    assert_eq!(first.len(), 24);
    assert_eq!(first[0]["mirror"], 1);
    assert_eq!(
        first[12]["mirror"], 1,
        "the same episode at the other hoster"
    );
    assert_eq!(tried["package_name"], "The Beginning After the End");

    // Saved, switched on, and listed with the document it was given.
    ok(
        &router,
        &session,
        "create_site_rule",
        serde_json::json!({ "rule": rule, "enabled": true }),
    )
    .await;
    let listed = ok(&router, &session, "list_site_rules", serde_json::json!({})).await;
    let stored = listed["rules"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == "warez-cx"))
        .unwrap_or_else(|| panic!("the rule is not listed: {listed}"));
    assert_eq!(stored["rule"], rule, "stored as written, groups included");
    assert_eq!(stored["active"], true);

    // In force for the next paste: the crawler selection turns the page into the
    // LinkGrabber's packages -- one per release -- and its mirror groups.
    let probe = Url::parse(PROBE).expect("probe");
    assert!(rules.claims(&probe));
    let rd_plugin_ext::CrawlOutcome::Links(found) = crawlers.expand(&probe, &HashMap::new()).await
    else {
        panic!("the saved rule did not answer for its probe");
    };
    assert_eq!(found.len(), 54);
    let mut packages: Vec<&str> = found
        .iter()
        .filter_map(|link| link.package_hint.as_deref())
        .collect();
    packages.dedup();
    assert_eq!(packages, names);
    let mut sets: HashMap<&str, usize> = HashMap::new();
    for link in &found {
        let key = link
            .mirror
            .as_ref()
            .map(|hint| hint.group.as_str())
            .unwrap_or_else(|| panic!("{} has no mirror group", link.url));
        *sets.entry(key).or_default() += 1;
    }
    assert_eq!(sets.len(), 27, "12 + 13 + 1 + 1 files");
    assert!(sets.values().all(|members| *members == 2), "{sets:?}");
}

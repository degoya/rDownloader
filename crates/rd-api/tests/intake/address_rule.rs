//! The address rule on every way a stranger chooses an address (RD-1190-18).
//!
//! RD-150-03 held a Metalink's mirrors and a crawler's finds on a stranger's page to the rule:
//! never this machine, the person's own network only when they handed the document over. A link
//! sent in as a link counted as the person's own whichever way it came, and a capture token
//! named its own way in. Here the online check of a link that Click'n'Load, the clipboard or the
//! extension handed over, of one a capture token claims to be the person's paste, and of one a
//! site rule found on a release page the person pasted, never requests this machine -- while
//! the person's own paste of the same address is checked as it always was. And a storage
//! profile cannot be pointed at a cloud's metadata endpoint.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use axum::http::StatusCode;
use rd_siterules::{Catalogue, Crawl, CrawlGroup, PickList, Rule, RunError};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

use crate::common::{self, eventually, post_capture, post_json, post_with_bearer};

/// A listener on this machine that answers every request with a small file and records the
/// path of each request -- the service's own port, in the real attack.
async fn listener() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&paths);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 2048];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or_default();
                seen.lock().expect("paths").push(path.to_owned());
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                          content-length: 4\r\nconnection: close\r\n\r\nfile",
                    )
                    .await;
            });
        }
    });
    (port, paths)
}

/// The candidate under `url` once its online check has settled.
async fn checked(database: &rd_db::Database, url: &str) -> rd_core::LinkCandidate {
    let url: Url = url.parse().expect("url");
    eventually(Duration::from_secs(10), "the link was checked", || {
        let url = url.clone();
        async move {
            database
                .list_candidates()
                .await
                .ok()?
                .into_iter()
                .find(|candidate| {
                    candidate.url == url
                        && candidate.checked_at.is_some()
                        && candidate.state != rd_core::LinkCandidateState::Checking
                })
        }
    })
    .await
}

fn requested(paths: &Arc<Mutex<Vec<String>>>, path: &str) -> bool {
    paths.lock().expect("paths").iter().any(|seen| seen == path)
}

/// Click'n'Load and the clipboard are filled by any web page, and the extension passes on what
/// a page chose: a link from any of them that points at this machine is marked and never
/// requested. The person's own paste of an address on the same listener is checked as always,
/// which is what makes the silence mean something.
#[tokio::test]
async fn a_link_a_page_handed_over_never_makes_the_service_ask_this_machine() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (port, paths) = listener().await;
    for source in ["click_and_load", "clipboard", "browser_extension"] {
        let url = format!("http://127.0.0.1:{port}/{source}.bin");
        let (status, body) = post_capture(
            &harness.router,
            json!({ "text": url, "source": source, "source_label": "Capture-Agent" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let candidate = checked(&harness.database, &url).await;
        assert_eq!(
            candidate.error_code.as_deref(),
            Some("collector.check_internal_address"),
            "{source}: {candidate:?}"
        );
    }
    let own = format!("http://127.0.0.1:{port}/own.bin");
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/batches",
        json!({ "text": own, "source": "manual" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let candidate = checked(&harness.database, &own).await;
    assert_ne!(
        candidate.error_code.as_deref(),
        Some("collector.check_internal_address")
    );
    assert!(requested(&paths, "/own.bin"));
    for source in ["click_and_load", "clipboard", "browser_extension"] {
        assert!(!requested(&paths, &format!("/{source}.bin")), "{source}");
    }
}

/// A capture token -- the extension's as well -- used to name its own way in, and `manual`
/// made its links the person's own hand. The capture door now records them as `api`.
#[tokio::test]
async fn a_capture_token_cannot_claim_the_persons_own_hand() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (port, paths) = listener().await;
    let url = format!("http://127.0.0.1:{port}/forged.bin");
    let (status, body) = post_capture(
        &harness.router,
        json!({ "text": url, "source": "manual", "source_label": "Capture-Agent" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["batch"]["source"], "api", "{body}");
    let candidate = checked(&harness.database, &url).await;
    assert_eq!(
        candidate.error_code.as_deref(),
        Some("collector.check_internal_address"),
        "{candidate:?}"
    );
    assert!(!requested(&paths, "/forged.bin"));
}

/// RD-1190-22: an API token on the LinkGrabber route is a program as well; naming `manual`
/// there no longer makes its links the person's own. The login is on, since a switched-off
/// login makes every local caller the administrator, bearer or not.
#[tokio::test]
async fn an_api_token_cannot_claim_the_persons_own_hand() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let (port, paths) = listener().await;
    let url = format!("http://127.0.0.1:{port}/token.bin");
    let (status, body) = post_with_bearer(
        &harness.router,
        "/api/v1/collector/batches",
        common::API_BEARER,
        json!({ "text": url, "source": "manual" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["batch"]["source"], "api", "{body}");
    let candidate = checked(&harness.database, &url).await;
    assert_eq!(
        candidate.error_code.as_deref(),
        Some("collector.check_internal_address"),
        "{candidate:?}"
    );
    assert!(!requested(&paths, "/token.bin"));
}

/// A release page's rule; the runner below stands in for the executor.
fn rule() -> Rule {
    serde_json::from_value(json!({
        "id": "board",
        "name": "board.example",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["board.example"] },
        "steps": [{ "kind": "fetch" },
                  { "kind": "regex", "pattern": "href=\"([^\"]+)\"", "into": "links", "all": true }],
        "package": { "from": "title" },
        "probe": "https://board.example/release/1",
        "checked": "2026-10-08"
    }))
    .expect("rule")
}

/// Finds one link on the page, at this machine. The executor drops a *literal* private address;
/// a name that resolves to one passes it, and this answer stands for that.
struct FindsLoopback(String);

#[async_trait]
impl rd_plugin_ext::RuleRunner for FindsLoopback {
    async fn run(&self, _rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        Ok(Crawl {
            address: address.clone(),
            links: vec![self.0.clone()],
            package_name: Some("Release".to_owned()),
            pages_fetched: 1,
            mirrors: false,
            groups: Vec::new(),
            pick: None,
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

/// The person pasted the release page, but its operator chose the links on it: a site rule's
/// find that points at this machine is neither probed by the crawl nor checked.
#[tokio::test]
async fn a_site_rules_find_never_makes_the_service_ask_this_machine() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (port, paths) = listener().await;
    let found = format!("http://127.0.0.1:{port}/found.bin");
    let rules = Arc::new(rd_plugin_ext::SiteRules::new(
        Catalogue::new(vec![rule()]),
        Arc::new(FindsLoopback(found.clone())),
    ));
    let crawlers = Arc::new(rd_plugin_ext::FolderCrawlers::none().with_rules(rules));
    let router = rd_api::router(harness.state.clone().with_crawlers(crawlers));
    let (status, body) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({ "text": "https://board.example/release/1", "source": "manual" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let candidate = checked(&harness.database, &found).await;
    assert_eq!(
        candidate.error_code.as_deref(),
        Some("collector.check_internal_address"),
        "{candidate:?}"
    );
    assert!(!requested(&paths, "/found.bin"));
}

/// RD-1190-22: the editor's trial run probed a rule's finds without the address rule, so a rule
/// that finds an address on this machine made *Test* request it.
#[tokio::test]
async fn a_trial_run_never_probes_this_machine() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (port, paths) = listener().await;
    let found = format!("http://127.0.0.1:{port}/trial.bin");
    let rules = Arc::new(rd_plugin_ext::SiteRules::new(
        Catalogue::new(vec![rule()]),
        Arc::new(FindsLoopback(found.clone())),
    ));
    let crawlers = Arc::new(rd_plugin_ext::FolderCrawlers::none().with_rules(rules));
    let router = rd_api::router(harness.state.clone().with_crawlers(crawlers));
    let (status, body) = post_json(
        &router,
        "/api/v1/site-rules/test",
        json!({
            "rule": serde_json::to_value(rule()).expect("rule"),
            "address": "https://board.example/release/1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["links"][0]["url"], found, "{body}");
    assert!(!requested(&paths, "/trial.bin"));
}

/// A profile's endpoint is an address the person entered: their MinIO beside the service is
/// fine, a cloud's metadata endpoint is refused when the profile is saved.
#[tokio::test]
async fn a_storage_profile_cannot_point_at_the_metadata_endpoint() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let profile = |name: &str, endpoint: &str| {
        json!({
            "name": name, "provider": "s3", "endpoint": endpoint, "region": "us-east-1",
            "bucket": "media", "credential_source": "anonymous"
        })
    };
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/object-storage/profiles",
        profile("metadata", "http://169.254.169.254"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "object_storage.endpoint_refused", "{body}");
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/object-storage/profiles",
        profile("minio", "http://127.0.0.1:9000"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

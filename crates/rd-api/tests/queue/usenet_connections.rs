//! RD-1270-21: the Usenet connections at work, as the rates and the server list answer them.
//!
//! The counting itself is `crates/rd-usenet/src/connections_tests.rs`; a runner that reports a
//! fixed reading stands in for it here, so what is checked is the way through the queue to the
//! response fields.

use std::sync::{Arc, Mutex};

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json};
use serde_json::json;

/// Reports whatever reading the test put in and runs nothing.
#[derive(Default)]
struct ReportingRunner {
    reading: Mutex<rd_scheduler::LiveConnections>,
}

#[async_trait::async_trait]
impl rd_scheduler::ExternalRunner for ReportingRunner {
    fn kind(&self) -> rd_core::DownloadKind {
        rd_core::DownloadKind::Usenet
    }

    fn slot_capacity(&self) -> usize {
        0
    }

    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: false,
            recheck_partial: false,
            adopt_completed: false,
            verify_completed: false,
            applies_collision_policy: false,
        }
    }

    fn live_connections(&self) -> Option<rd_scheduler::LiveConnections> {
        Some(
            self.reading
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    async fn run(
        &self,
        _file: &rd_core::DownloadFile,
        _package: &rd_core::DownloadPackage,
        _cancellation: tokio_util::sync::CancellationToken,
        _limits: rd_scheduler::RunLimits,
    ) -> anyhow::Result<rd_scheduler::RunOutcome> {
        Ok(rd_scheduler::RunOutcome::Stopped)
    }
}

#[tokio::test]
async fn rates_and_server_list_carry_the_connections_at_work() {
    let directory = tempfile::tempdir().expect("tempdir");
    let runner = Arc::new(ReportingRunner::default());
    let harness = common::harness(
        directory.path(),
        common::Options::default()
            .parked()
            .runner(Arc::clone(&runner) as Arc<dyn rd_scheduler::ExternalRunner>),
    )
    .await;
    let mut files = Vec::new();
    for url in [
        "https://example.test/one.bin",
        "https://example.test/two.bin",
        "https://example.test/idle.bin",
    ] {
        let (status, created) = post_json(
            &harness.router,
            "/api/v1/downloads",
            json!({ "url": url, "paused": true }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        files.push(
            created["id"]
                .as_str()
                .expect("download id")
                .parse::<rd_core::DownloadId>()
                .expect("id"),
        );
    }
    let (status, server) = post_json(
        &harness.router,
        "/api/v1/usenet/servers",
        json!({
            "name": "primary",
            "host": "news.example.test",
            "port": 563,
            "tls": true,
            "priority": 10,
            "max_connections": 10,
            "enabled": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    let server_id = server["id"]
        .as_str()
        .expect("server id")
        .parse::<rd_core::UsenetServerId>()
        .expect("id");
    // Two files sharing a server's ten connections, five each; the third file rests.
    *runner
        .reading
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = rd_scheduler::LiveConnections {
        files: [(files[0], 5), (files[1], 5)].into_iter().collect(),
        servers: vec![rd_scheduler::ServerConnections {
            server: server_id,
            busy: 10,
            limit: 10,
        }],
    };

    let (status, rates) = get_json(&harness.router, "/api/v1/downloads/rates").await;
    assert_eq!(status, StatusCode::OK, "{rates}");
    let entries = rates["downloads"].as_array().expect("entries");
    for file in &files[..2] {
        let entry = entries
            .iter()
            .find(|entry| entry["id"] == json!(file.to_string()))
            .unwrap_or_else(|| panic!("a file holding connections is listed: {rates}"));
        assert_eq!(entry["connections"], json!(5));
        assert_eq!(entry["bytes_per_second"], json!(0));
    }
    assert!(
        entries
            .iter()
            .all(|entry| entry["id"] != json!(files[2].to_string())),
        "a file at rest without connections stays out: {rates}"
    );
    assert_eq!(
        rates["usenet_servers"],
        json!([{ "id": server_id.to_string(), "busy": 10, "limit": 10 }])
    );

    let (status, servers) = get_json(&harness.router, "/api/v1/usenet/servers").await;
    assert_eq!(status, StatusCode::OK, "{servers}");
    assert_eq!(servers[0]["busy_connections"], json!(10));
}

#[tokio::test]
async fn without_a_pool_nothing_is_busy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (status, rates) = get_json(&harness.router, "/api/v1/downloads/rates").await;
    assert_eq!(status, StatusCode::OK, "{rates}");
    assert_eq!(rates["usenet_servers"], json!([]));
    let (status, server) = post_json(
        &harness.router,
        "/api/v1/usenet/servers",
        json!({
            "name": "primary",
            "host": "news.example.test",
            "port": 563,
            "tls": true,
            "priority": 10,
            "max_connections": 10,
            "enabled": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    let (status, servers) = get_json(&harness.router, "/api/v1/usenet/servers").await;
    assert_eq!(status, StatusCode::OK, "{servers}");
    assert_eq!(servers[0]["busy_connections"], json!(0));
}

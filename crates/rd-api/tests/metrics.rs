//! The metrics exposition and the transfer statistics (RD-110-01).
//!
//! Four things are proven here: an unauthenticated request gets nothing; the scrape scope is
//! an island in both directions; the text is a valid exposition; and the label set does not
//! grow with the queue and carries nothing a person typed.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{
    API_BEARER, READ_BEARER, auth_harness, get_json, get_with_bearer, put_json, test_harness,
};
use http_body_util::BodyExt;
use rd_core::{AuthProfileSelection, DownloadId, DownloadKind, DownloadState, PackageId};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const METRICS_BEARER: &str = "test-metrics-bearer-token";

/// Mints a bearer holding exactly `api:metrics`.
async fn metrics_token(database: &rd_db::Database) {
    let result = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "prometheus".to_owned(),
            hex::encode(Sha256::digest(METRICS_BEARER.as_bytes())),
            vec![rd_core::API_METRICS_SCOPE.to_owned()],
        )
        .await;
    if let Err(error) = result {
        assert!(
            error.to_string().contains("UNIQUE constraint failed"),
            "minting: {error}"
        );
    }
}

/// `GET /api/v1/metrics` as text, with an optional bearer and `Accept`.
async fn scrape(
    router: &Router,
    bearer: Option<&str>,
    accept: Option<&str>,
) -> (StatusCode, String, String) {
    let mut request = Request::builder()
        .method("GET")
        .uri("/api/v1/metrics")
        .header(header::HOST, "127.0.0.1:8710");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// Rows the harness's real scheduler leaves alone.
///
/// `Queued` would be picked up within milliseconds and driven through `resolving` into
/// `retry_wait` — a DNS failure against `example.test` — so the states the assertions name,
/// and with them the series set, depended on which test body the scheduler was serving at
/// the moment of the scrape. `Paused` is a state the scheduler never leaves on its own.
async fn queue_downloads(
    database: &rd_db::Database,
    directory: &std::path::Path,
    count: usize,
    seed: usize,
) {
    let package = database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: format!("Private package {seed}"),
            destination: directory.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for index in 0..count {
        let number = seed + index;
        database
            .create_download(rd_db::NewDownload {
                id: DownloadId::new(),
                package_id: package.id,
                source: format!("https://host-{number}.example.test/secret-file-{number}.bin")
                    .parse()
                    .expect("URL"),
                file_name: format!("secret-file-{number}.bin"),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: DownloadState::Paused,
                kind: DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
    }
}

/// The sample lines of an exposition: neither comments nor blank.
fn series(body: &str) -> Vec<&str> {
    body.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// Without a credential the route says nothing at all, not even a family name.
#[tokio::test]
async fn an_unauthenticated_request_gets_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let (status, _, body) = scrape(&harness.router, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(!body.contains("rdownloader_"), "{body}");
}

/// The scrape scope reaches the exposition and nothing else; nothing short of `api:*`
/// reaches the exposition.
#[tokio::test]
async fn the_metrics_scope_is_an_island() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    metrics_token(&harness.database).await;

    let (status, content_type, body) = scrape(&harness.router, Some(METRICS_BEARER), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        content_type.starts_with("text/plain; version=0.0.4"),
        "{content_type}"
    );

    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/downloads", METRICS_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient");
    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/stats/transfers", METRICS_BEARER).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a scrape token must not read the statistics either: {body}"
    );
    let (status, _) = get_with_bearer(&harness.router, "/mcp", METRICS_BEARER).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a scrape token must not open an MCP session"
    );

    let (status, _, body) = scrape(&harness.router, Some(READ_BEARER), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "api:read must not scrape: {body}"
    );
    let (status, _, _) = scrape(&harness.router, Some(API_BEARER), None).await;
    assert_eq!(status, StatusCode::OK, "api:* still reaches everything");

    // The policy table agrees: exactly one route costs the scope.
    let costing_metrics: Vec<_> = rd_api::policy_rows()
        .into_iter()
        .filter(|(_, _, required)| *required == Some(rd_core::API_METRICS_SCOPE))
        .collect();
    assert_eq!(costing_metrics.len(), 1, "{costing_metrics:?}");
    assert_eq!(costing_metrics[0].0, "/api/v1/metrics");
}

/// A valid exposition: families announced before their samples, every sample line
/// well-formed, `# EOF` last, and the OpenMetrics media type on request.
#[tokio::test]
async fn prometheus_can_scrape_a_valid_exposition() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    queue_downloads(&harness.database, directory.path(), 2, 0).await;

    let (status, content_type, body) = scrape(
        &harness.router,
        None,
        Some("application/openmetrics-text;version=1.0.0,text/plain;version=0.0.4;q=0.5"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        content_type,
        "application/openmetrics-text; version=1.0.0; charset=utf-8"
    );
    assert_eq!(body.lines().last(), Some("# EOF"));

    let sample = regex::Regex::new(
        r#"^[a-zA-Z_:][a-zA-Z0-9_:]*(\{([a-zA-Z_][a-zA-Z0-9_]*="[^"\n]*")(,[a-zA-Z_][a-zA-Z0-9_]*="[^"\n]*")*\})? (-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?|\+Inf|-Inf|NaN)$"#,
    )
    .expect("pattern");
    let mut announced = std::collections::BTreeSet::new();
    let mut typed = std::collections::BTreeSet::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("# HELP ") {
            announced.insert(rest.split(' ').next().unwrap_or_default().to_owned());
        } else if let Some(rest) = line.strip_prefix("# TYPE ") {
            let mut parts = rest.split(' ');
            let name = parts.next().unwrap_or_default();
            let kind = parts.next().unwrap_or_default();
            assert!(matches!(kind, "gauge" | "counter" | "histogram"), "{line}");
            assert!(announced.contains(name), "TYPE before HELP: {line}");
            typed.insert(name.to_owned());
        } else if line == "# EOF" || line.is_empty() {
            continue;
        } else {
            assert!(sample.is_match(line), "malformed sample line: {line}");
            let name = line.split(['{', ' ']).next().unwrap_or_default();
            let family = name
                .strip_suffix("_bucket")
                .or_else(|| name.strip_suffix("_sum"))
                .or_else(|| name.strip_suffix("_count"))
                .unwrap_or(name);
            assert!(
                typed.contains(family),
                "sample before its TYPE line: {line}"
            );
        }
    }
    for family in [
        "rdownloader_build_info",
        "rdownloader_uptime_seconds",
        "rdownloader_queue_downloads",
        "rdownloader_queue_wait_seconds",
        "rdownloader_runners_active",
        "rdownloader_transfer_rate_bytes_per_second",
        "rdownloader_transfers_total",
        "rdownloader_transfer_bytes_total",
        "rdownloader_transfer_retries_total",
        "rdownloader_transfer_seconds_total",
        "rdownloader_provider_accounts",
        "rdownloader_provider_hosts_blocked",
        "rdownloader_storage_free_bytes",
        "rdownloader_storage_total_bytes",
        "rdownloader_storage_blocked",
    ] {
        assert!(
            typed.contains(family),
            "{family} is missing from the exposition"
        );
    }
    assert!(
        body.contains("rdownloader_queue_downloads{kind=\"http\",state=\"paused\"} 2"),
        "{body}"
    );
    // Paused rows are not waiting for a slot, so the histogram is present and empty.
    assert!(
        body.contains("rdownloader_queue_wait_seconds_bucket{le=\"+Inf\"} 0"),
        "{body}"
    );
    assert!(
        body.contains("rdownloader_queue_wait_seconds_count 0"),
        "{body}"
    );
}

/// Three hundred more downloads, each with its own address and name, add not one series —
/// and none of the names, the package label, the account label or the user name is in the
/// text.
#[tokio::test]
async fn a_large_queue_does_not_grow_the_series_set_and_no_name_reaches_a_label() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    harness
        .database
        .create_account(rd_db::NewAccount {
            provider: "rapidgator".to_owned(),
            label: "Alexanders Konto".to_owned(),
            username: Some("alex.private@example.test".to_owned()),
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    queue_downloads(&harness.database, directory.path(), 3, 0).await;
    let (_, _, small) = scrape(&harness.router, None, None).await;
    queue_downloads(&harness.database, directory.path(), 300, 1_000).await;
    let (_, _, large) = scrape(&harness.router, None, None).await;

    // The rate gauge gains its one series per kind at the scheduler's first sampling pass,
    // whenever that falls; on a slow runner it fell between the two scrapes (2026-09-25).
    let without_rate = |body: &str| {
        series(body)
            .into_iter()
            .filter(|line| !line.starts_with("rdownloader_transfer_rate_bytes_per_second"))
            .count()
    };
    assert_eq!(
        without_rate(&small),
        without_rate(&large),
        "the series set grew with the queue:\n{large}"
    );
    assert!(
        large
            .lines()
            .filter(|line| line.starts_with("rdownloader_transfer_rate_bytes_per_second{"))
            .all(|line| line
                .starts_with("rdownloader_transfer_rate_bytes_per_second{kind=\"http\"}")),
        "{large}"
    );
    assert!(
        large.contains("rdownloader_queue_downloads{kind=\"http\",state=\"paused\"} 303"),
        "{large}"
    );
    assert!(
        large.contains("rdownloader_provider_accounts{provider=\"rapidgator\",enabled=\"true\"} 1")
    );
    for forbidden in [
        "secret-file",
        "host-1",
        "example.test",
        "Private package",
        "Alexanders",
        "alex.private",
        directory.path().to_string_lossy().as_ref(),
    ] {
        assert!(
            !large.contains(forbidden),
            "{forbidden} reached the exposition:\n{large}"
        );
    }
    // Every label value is drawn from a closed vocabulary: enum names, ids and `direct`.
    let label = regex::Regex::new(r#"([a-z_]+)="([^"]*)""#).expect("pattern");
    for capture in label.captures_iter(&large) {
        let (name, value) = (&capture[1], &capture[2]);
        assert!(
            value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')),
            "label {name} carries free text: {value}"
        );
    }
}

/// The statistics are a dashboard's reading, so `api:read` gets them, and the settings
/// refuse a retention that is out of range.
#[tokio::test]
async fn transfer_stats_are_readable_and_the_retention_is_bounded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let (status, body) = get_with_bearer(
        &harness.router,
        "/api/v1/stats/transfers?range=week",
        READ_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["range"], "week");
    assert_eq!(body["resolution"], "day");
    assert!(body["buckets"].is_array());
    assert_eq!(body["totals"]["completed"], 0);
    let (status, body) = get_with_bearer(
        &harness.router,
        "/api/v1/stats/transfers?range=decade",
        READ_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let open = test_harness(directory.path()).await;
    let (status, mut settings) = get_json(&open.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["stats_hourly_days"], 30);
    assert_eq!(settings["stats_retention_days"], 365);
    settings["stats_retention_days"] = serde_json::json!(3);
    let (status, body) = put_json(&open.router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.stats_retention_invalid");
    settings["stats_retention_days"] = serde_json::json!(90);
    settings["stats_hourly_days"] = serde_json::json!(7);
    let (status, body) = put_json(&open.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stats_hourly_days"], 7);
}

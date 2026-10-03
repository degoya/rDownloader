//! Secret canaries (RD-170-06): a distinctive value planted as every kind of credential the
//! service keeps, then searched for everywhere the service hands something out.
//!
//! Each value goes in through the route the interface uses for it ([`PLANTINGS`]): the
//! administrator password, an API key filling a plugin's secret slot and a login password on
//! two provider accounts, the account's cookies, a proxy password, an object storage secret key
//! and session token, an FTP password, a webhook signing secret, an ntfy token inside an apprise
//! URL, an NNTP password, an authentication profile's token, the captcha solver's API key (a
//! secret the settings table refers to), the full-backup passphrase, a settings-export
//! passphrase, a minted API token, and the identity provider's client secret with the access,
//! refresh and ID tokens its token endpoint answers a link and a sign-in with
//! ([`identity_provider`]). The checks that send a credential somewhere are then made
//! to fail against fixtures that refuse them ([`FAILURES`]), so the error paths answer and log
//! with the credential in reach.
//!
//! Then every place is read: the REST reads and MCP tools of [`PLACES`], the answers of the
//! planting and failing requests, the web event stream of the whole run, every line the log
//! capture took (the layer the viewer and the bundle read from, under the service's filter one
//! level deeper), the raw full-backup archive and each of its parts once decrypted, and each
//! entry of a diagnostic bundle. A hit names the canary and the place. A new place is one line
//! in [`PLACES`]; a new credential is a name in [`CANARIES`] and a row in [`PLANTINGS`].

use crate::common::{self, Harness};

mod identity_provider;
mod support;

use std::{path::Path, sync::Arc, time::Duration};

use axum::{Router, http::StatusCode};
use rd_diagnostics::capture::{CaptureStats, LogCaptureLayer};
use serde_json::{Value, json};
use support::{
    Planted, Scan, call_tool, contains, data_directory, drain, expect_ok, install_providers,
    listen, mcp_session, refusing_ftp, refusing_http, rest,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, registry};

/// Every secret planted, by the name a finding reports it under. `api-token` joins once the
/// service has minted it.
const CANARIES: &[&str] = &[
    "admin-password",
    "admin-wrong-password",
    "account-api-key",
    "account-cookies",
    "account-password",
    "proxy-password",
    "s3-secret-key",
    "s3-session-token",
    "ftp-password",
    "webhook-secret",
    "ntfy-token",
    "nntp-password",
    "auth-profile-token",
    "captcha-api-key",
    "backup-passphrase",
    "backup-wrong-passphrase",
    "backup-refused-passphrase",
    "export-passphrase",
    "oidc-client-secret",
    "oidc-access-token",
    "oidc-refresh-token",
];

/// The planting requests, in order: the name the answer's `id` is kept under (`-` for none),
/// method, route and body. `{name}` is a canary, a kept id, `{refusing}` (an HTTP fixture that
/// refuses every request) or `{ftp-port}` (an FTP fixture that refuses every login).
const PLANTINGS: &[(&str, &str, &str, &str)] = &[
    (
        "proxy",
        "POST",
        "/api/v1/proxy-profiles",
        r#"{"name": "canary-proxy", "kind": "socks5", "endpoint": "socks5h://127.0.0.1:1080",
            "username": "canary-proxy-user", "password": "{proxy-password}"}"#,
    ),
    (
        "key-account",
        "POST",
        "/api/v1/accounts",
        r#"{"provider": "canarykey", "label": "canary-key-account", "username": null,
            "credential_mode": null, "secret": "{account-api-key}",
            "cookies": "{account-cookies}", "proxy_profile_id": "{proxy}", "enabled": true}"#,
    ),
    (
        "login-account",
        "POST",
        "/api/v1/accounts",
        r#"{"provider": "canarylogin", "label": "canary-login-account",
            "username": "canary-login-user", "credential_mode": null,
            "secret": "{account-password}", "cookies": null, "proxy_profile_id": null,
            "enabled": true}"#,
    ),
    (
        "bucket",
        "POST",
        "/api/v1/object-storage/profiles",
        r#"{"name": "canary-bucket-profile", "provider": "s3", "endpoint": "{refusing}",
            "region": "us-east-1", "bucket": "canary-bucket", "credential_source": "static",
            "access_key_id": "AKIACANARYFIXTURE", "secret_access_key": "{s3-secret-key}",
            "session_token": "{s3-session-token}"}"#,
    ),
    (
        "ftp",
        "POST",
        "/api/v1/remote-credentials",
        r#"{"name": "canary-ftp", "protocol": "ftp", "host": "127.0.0.1", "port": {ftp-port},
            "username": "canary-ftp-user", "auth_mode": "password", "secret": "{ftp-password}"}"#,
    ),
    (
        "webhook",
        "POST",
        "/api/v1/notifications/targets",
        r#"{"name": "canary-webhook", "kind": "webhook", "endpoint": "{refusing}/hook",
            "secret": "{webhook-secret}"}"#,
    ),
    (
        "-",
        "POST",
        "/api/v1/notifications/targets",
        r#"{"name": "canary-apprise", "kind": "apprise", "endpoint": "ntfys",
            "secret": "ntfys://{ntfy-token}@ntfy.canary.test/canaries"}"#,
    ),
    (
        "-",
        "POST",
        "/api/v1/usenet/servers",
        r#"{"name": "canary-news", "host": "news.canary.test", "port": 563, "tls": true,
            "username": "canary-reader", "password": "{nntp-password}", "proxy_profile_id": null,
            "priority": 0, "max_connections": 2, "enabled": false}"#,
    ),
    (
        "-",
        "POST",
        "/api/v1/auth-profiles",
        r#"{"name": "canary-auth-profile", "scope": "files.canary.test",
            "include_subdomains": false, "method": "bearer", "username": null,
            "secret": "{auth-profile-token}", "certificate_pem": null, "expires_at": null,
            "enabled": true}"#,
    ),
    (
        "-",
        "PUT",
        "/api/v1/captcha-config",
        r#"{"solver": "two_captcha_compatible", "endpoint": "https://captcha.canary.test",
            "api_key": "{captcha-api-key}"}"#,
    ),
    (
        "-",
        "PUT",
        "/api/v1/backups/passphrase",
        r#"{"passphrase": "{backup-passphrase}"}"#,
    ),
];

/// The requests that send or check a planted credential, each made to fail: method, route and
/// body, expanded like [`PLANTINGS`]. Their answers are searched whatever their status.
const FAILURES: &[(&str, &str, Option<&str>)] = &[
    (
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"password": "{admin-wrong-password}"}"#),
    ),
    ("POST", "/api/v1/remote-credentials/{ftp}/test", None),
    (
        "POST",
        "/api/v1/object-storage/profiles/{bucket}/test",
        None,
    ),
    ("POST", "/api/v1/notifications/targets/{webhook}/test", None),
    (
        "POST",
        "/api/v1/captcha-config/test",
        Some(r#"{"endpoint": "https://127.0.0.1:1"}"#),
    ),
    ("POST", "/api/v1/accounts/{key-account}/test", None),
    ("POST", "/api/v1/accounts/{login-account}/test", None),
    ("GET", "/api/v1/accounts/{key-account}/auth", None),
    (
        "GET",
        "/api/v1/accounts/{key-account}/browser-session",
        None,
    ),
    ("GET", "/api/v1/accounts/{key-account}/hosters", None),
    (
        "PUT",
        "/api/v1/backups/passphrase",
        Some(
            r#"{"passphrase": "{backup-refused-passphrase}",
                "current_passphrase": "{backup-wrong-passphrase}"}"#,
        ),
    ),
];

/// How one place is read, expanded like [`PLANTINGS`].
enum Read {
    /// `GET` with the full-access bearer.
    Get(&'static str),
    /// `POST` with the full-access bearer and this JSON body.
    Post(&'static str, &'static str),
    /// An MCP tool with these JSON arguments.
    Tool(&'static str, &'static str),
}

/// Every place read once everything is planted. Each has to answer: a mistyped route fails the
/// test instead of searching an error page.
const PLACES: &[Read] = &[
    Read::Get("/api/v1/accounts"),
    Read::Get("/api/v1/proxy-profiles"),
    Read::Get("/api/v1/object-storage/profiles"),
    Read::Get("/api/v1/remote-credentials"),
    Read::Get("/api/v1/remote-credentials/ssh-hosts"),
    Read::Get("/api/v1/notifications/targets"),
    Read::Get("/api/v1/notifications/deliveries?limit=500"),
    Read::Get("/api/v1/usenet/servers"),
    Read::Get("/api/v1/auth-profiles"),
    Read::Get("/api/v1/api-tokens"),
    Read::Get("/api/v1/captcha-config"),
    Read::Get("/api/v1/providers"),
    Read::Get("/api/v1/settings"),
    Read::Get("/api/v1/auth/status"),
    Read::Get("/api/v1/auth/oidc"),
    Read::Get("/api/v1/setup/status"),
    Read::Get("/api/v1/openapi.json"),
    Read::Get("/api/v1/backups"),
    Read::Get("/api/v1/backups/runs"),
    Read::Get("/api/v1/backups/archives"),
    Read::Get("/api/v1/audit/records?limit=500"),
    Read::Get("/api/v1/audit/export"),
    Read::Get("/api/v1/diagnostics/logs?limit=500"),
    Read::Get("/api/v1/routing/export"),
    Read::Get("/api/v1/system/about"),
    Read::Post("/api/v1/settings/export", r#"{"include_secrets": false}"#),
    Read::Post(
        "/api/v1/settings/export",
        r#"{"include_secrets": true, "passphrase": "{export-passphrase}"}"#,
    ),
    Read::Tool("list_configuration", r#"{"section": "accounts"}"#),
    Read::Tool("list_configuration", r#"{"section": "proxy_profiles"}"#),
    Read::Tool("list_configuration", r#"{"section": "providers"}"#),
    Read::Tool("list_configuration", r#"{"section": "plugins"}"#),
    Read::Tool("list_usenet_servers", "{}"),
    Read::Tool("list_notification_targets", "{}"),
    Read::Tool("list_notification_deliveries", "{}"),
    Read::Tool("get_settings", "{}"),
    Read::Tool("get_status_summary", "{}"),
    Read::Tool("get_backup_status", "{}"),
    Read::Tool("list_backup_runs", "{}"),
    Read::Tool("list_backup_archives", "{}"),
    Read::Tool("list_log_records", r#"{"limit": 500}"#),
    Read::Tool("list_audit_records", r#"{"limit": 500}"#),
];

/// The service's own filter (`init_tracing`), one level deeper: a debug line carrying a
/// credential is a leak too once somebody turns debug on for a support case.
const LOG_FILTER: &str = "rdownloader=debug,rd_=debug";

#[tokio::test]
async fn no_planted_secret_comes_back_out_anywhere() {
    data_directory();
    let (layer, mut stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    // Thread-local, and a `tokio::test` runtime is this one thread: every task the service
    // spawns logs through it.
    let _capture =
        tracing::subscriber::set_default(registry().with(EnvFilter::new(LOG_FILTER)).with(layer));
    tracing::warn!(target: "rd_canaries", code = "canaries.witness", "the capture listens");
    let mut lines = Vec::new();
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let router = &harness.router;
    let mut planted = Planted::new();
    let mut scan = Scan::default();
    let (heard, listener) = listen(router).await;

    let session = plant(router, &mut planted, &mut scan).await;
    identity_provider::sign_in_through_provider(&harness, &session, &mut planted, &mut scan).await;
    drain(&mut stream, &harness.database, &mut lines).await;
    for (method, uri, body) in FAILURES {
        let uri = planted.expand(uri);
        let body = body.map(|body| serde_json::from_str(&planted.expand(body)).expect("JSON"));
        let (status, answer) = rest(router, method, &uri, body).await;
        scan.record(format!("{method} {uri} ({status})"), answer);
    }
    drain(&mut stream, &harness.database, &mut lines).await;
    back_up(&harness, &planted, directory.path(), &mut scan).await;
    drain(&mut stream, &harness.database, &mut lines).await;
    bundle(router, &mut scan).await;
    read_places(router, &planted, &mut scan).await;

    // The bus delivers in order: once this has arrived, everything before it has too.
    harness.database.broadcast(rd_core::EventEnvelope::new(
        rd_core::EventKind::CollectorIntake,
        json!({ "candidate_count": 0, "package_count": 0, "source": "api" }),
    ));
    common::eventually(common::WAIT, "the closing event on the stream", || {
        let heard = Arc::clone(&heard);
        async move { contains(&heard.lock().expect("stream"), b"collector.intake").then_some(()) }
    })
    .await;
    listener.abort();
    let events = heard.lock().expect("stream").clone();
    scan.record("web event stream", events);
    drain(&mut stream, &harness.database, &mut lines).await;
    scan.record("log capture", lines);

    // Every search has to have had something to search.
    for row in [
        "canary-proxy",
        "canary-key-account",
        "canary-login-account",
        "canary-bucket-profile",
        "canary-ftp",
        "canary-webhook",
        "canary-apprise",
        "canary-news",
        "canary-auth-profile",
        "canary-token",
    ] {
        assert!(scan.shows("GET ", row), "no REST read showed the row {row}");
    }
    for row in [
        "canary-key-account",
        "canary-proxy",
        "canary-news",
        "canary-webhook",
    ] {
        assert!(scan.shows("mcp ", row), "no MCP tool showed the row {row}");
    }
    assert!(
        scan.shows("log capture", "canaries.witness"),
        "the capture took nothing"
    );
    assert!(
        scan.shows("backup part ", "canary-proxy"),
        "the backup holds no settings"
    );
    let entries = scan
        .0
        .iter()
        .filter(|(place, _)| place.starts_with("diagnostics bundle "));
    assert!(
        entries.count() >= 5,
        "the bundle has fewer entries than its preview"
    );

    let leaks = scan.leaks(&planted);
    assert!(
        leaks.is_empty(),
        "planted secrets came back out:\n{}",
        leaks.join("\n")
    );
}

/// Signs in, plants every row of [`PLANTINGS`] and mints an API token; returns the session.
async fn plant(router: &Router, planted: &mut Planted, scan: &mut Scan) -> String {
    let refusing = refusing_http().await;
    planted.known.push(("refusing".to_owned(), refusing));
    let ftp_port = refusing_ftp().await;
    planted
        .known
        .push(("ftp-port".to_owned(), ftp_port.to_string()));
    let session = common::sign_in(router, &planted.canary("admin-password")).await;
    install_providers();

    for (keep, method, uri, template) in PLANTINGS {
        let body = serde_json::from_str(&planted.expand(template)).expect("a planting body");
        let answer = expect_ok(router, scan, method, uri, body).await;
        if *keep != "-" {
            let id = answer["id"].as_str().expect("the planted row's id");
            planted.known.push(((*keep).to_owned(), id.to_owned()));
        }
    }

    // The one answer allowed to carry a secret: the bearer, shown once. Not recorded.
    let request = json!({ "label": "canary-token", "scopes": [] });
    let (status, minted) = rest(router, "POST", "/api/v1/api-tokens", Some(request)).await;
    let minted: Value = serde_json::from_slice(&minted).expect("minted token");
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let bearer = minted["bearer"].as_str().expect("bearer").to_owned();
    planted.canaries.push(("api-token", bearer));
    session
}

/// Runs a full backup and records the archive as it lies on disk, then every part once opened.
async fn back_up(harness: &Harness, planted: &Planted, directory: &Path, scan: &mut Scan) {
    let router = &harness.router;
    let folder = directory.join("nas");
    let destination = json!({ "kind": "local", "path": folder.display().to_string() });
    expect_ok(
        router,
        scan,
        "POST",
        "/api/v1/backups/destinations",
        destination,
    )
    .await;
    let schedule = json!({ "enabled": true, "schedule": "0 3 * * *", "timezone": "Europe/Berlin" });
    expect_ok(router, scan, "PUT", "/api/v1/backups", schedule).await;
    let run = expect_ok(router, scan, "POST", "/api/v1/backups/runs", json!({})).await;
    let id = run["id"].as_str().expect("run id");
    let database = &harness.database;
    let run = common::eventually(Duration::from_secs(60), "the backup run", || async move {
        database
            .backup_run(id)
            .await
            .expect("read run")
            .filter(|run| run.state != rd_core::BackupRunState::Running)
    })
    .await;
    assert_eq!(
        run.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        run.error_detail
    );
    let archive = folder.join(run.archive_name.as_deref().expect("archive name"));
    scan.record(
        "backup archive (raw)",
        std::fs::read(&archive).expect("archive"),
    );

    let header = rd_backup::stream::read_header(&archive).expect("header");
    let passphrase = planted.canary("backup-passphrase");
    let key = rd_backup::BackupKey::derive(&passphrase, header.salt)
        .await
        .expect("key");
    let opened = directory.join("opened");
    let manifest = rd_backup::archive::extract_archive(&archive, &key, &opened).expect("extract");
    scan.record(
        "backup manifest",
        serde_json::to_vec(&manifest).expect("manifest"),
    );
    let mut pending = vec![opened.clone()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).expect("opened folder") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(&opened)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                scan.record(
                    format!("backup part {name}"),
                    std::fs::read(&path).expect("part"),
                );
            }
        }
    }
}

/// Writes a diagnostic bundle of everything its preview offers and records each entry.
async fn bundle(router: &Router, scan: &mut Scan) {
    let preview_route = "/api/v1/diagnostics/bundle/preview";
    let preview = expect_ok(router, scan, "GET", preview_route, Value::Null).await;
    let entries: Vec<Value> = preview["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["id"].clone())
        .collect();
    let request = json!({ "approved": true, "digest": preview["digest"], "entries": entries });
    let created = expect_ok(router, scan, "POST", "/api/v1/diagnostics/bundle", request).await;
    let bytes = std::fs::read(created["path"].as_str().expect("path")).expect("bundle");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("entry");
        let name = entry.name().to_owned();
        let mut content = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut content).expect("read entry");
        scan.record(format!("diagnostics bundle {name}"), content);
    }
}

/// Reads every place of [`PLACES`]; each has to answer.
async fn read_places(router: &Router, planted: &Planted, scan: &mut Scan) {
    let session = mcp_session(router).await;
    let mut refused = Vec::new();
    for (index, place) in PLACES.iter().enumerate() {
        let (name, answered, bytes) = match place {
            Read::Get(uri) => {
                let (status, bytes) = rest(router, "GET", uri, None).await;
                (format!("GET {uri}"), status.is_success(), bytes)
            }
            Read::Post(uri, template) => {
                let body = serde_json::from_str(&planted.expand(template)).expect("a JSON body");
                let (status, bytes) = rest(router, "POST", uri, Some(body)).await;
                (format!("POST {uri} {template}"), status.is_success(), bytes)
            }
            Read::Tool(tool, arguments) => {
                let (answered, bytes) = call_tool(router, &session, index, tool, arguments).await;
                (format!("mcp {tool} {arguments}"), answered, bytes)
            }
        };
        if !answered {
            refused.push(format!("{name}: {}", String::from_utf8_lossy(&bytes)));
        }
        scan.record(name, bytes);
    }
    assert!(
        refused.is_empty(),
        "places that did not answer:\n{}",
        refused.join("\n")
    );
}

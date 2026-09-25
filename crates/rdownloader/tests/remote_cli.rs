//! The CLI against a real server on a real port.
//!
//! The router tests elsewhere use `oneshot`, which never opens a socket. This one has to:
//! what is being checked is the client half — the address, the bearer token, the way an
//! error response is turned into a message — and none of that exists without a listener.

use std::process::Command;

use tokio::net::TcpListener;

/// Starts a server on an ephemeral port and returns its address plus a shutdown token.
async fn serve(
    directory: &std::path::Path,
) -> (String, tokio_util::sync::CancellationToken, String) {
    let database = rd_db::Database::open(directory.join("cli.sqlite3"))
        .await
        .expect("database");
    let bearer = "cli-test-token".to_owned();
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "cli".to_owned(),
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bearer.as_bytes())),
            vec![rd_core::API_SCOPE.to_owned()],
        )
        .await
        .expect("token");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let plugins = rd_plugin_host::PluginInstaller::new(
        directory.join("plugins"),
        rd_plugin_host::PluginVerifier::new(true),
    );
    let media_settings = rd_media::shared_settings(&database).await.expect("media");
    let (_runner, media_probe) =
        rd_media::build(database.clone(), secrets.clone(), media_settings.clone());
    let gallery_settings = rd_gallery::shared_settings(&database)
        .await
        .expect("gallery");
    let stream_settings = rd_stream::shared_settings(&database).await.expect("stream");
    let torrent_settings = rd_torrent::shared_settings(&database)
        .await
        .expect("torrent");
    let torrent = rd_torrent::TorrentService::start(
        database.clone(),
        torrent_settings.clone(),
        directory.to_path_buf(),
        directory.join("downloads"),
    );
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.join("passwords.txt"),
            rar_timeout: std::time::Duration::from_secs(60),
            default_scripts_directory: directory.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    let remote = rd_api::RemoteServices::new(
        database.clone(),
        secrets.clone(),
        std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
        rd_http::SharedNetworkDefaults::default(),
    );
    let state = rd_api::AppState::new(
        database,
        scheduler,
        secrets,
        plugins,
        extraction,
        media_settings,
        media_probe,
        gallery_settings,
        stream_settings,
        torrent,
        torrent_settings,
        rd_power::PowerService::default(),
        rd_core::PostprocessHold::new(),
        remote,
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");
    let router = rd_api::router(state);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await;
    });
    (format!("http://{address}"), shutdown, bearer)
}

/// Runs the built binary with the given arguments.
fn run(args: &[&str]) -> (bool, String, String) {
    let (success, stdout, stderr, _) = run_with_code(args);
    (success, stdout, stderr)
}

/// Same, but keeps the exit code, which is the contract for scripts.
fn run_with_code(args: &[&str]) -> (bool, String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_rdownloader"))
        .args(args)
        .output()
        .expect("run rdownloader");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_queue_can_be_listed_added_to_and_controlled_over_http() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (server, shutdown, token) = serve(directory.path()).await;

    let (ok, stdout, stderr) = run(&["queue", "list", "--server", &server, "--token", &token]);
    assert!(ok, "queue list failed: {stderr}");
    assert!(stdout.contains("nothing to show"), "{stdout}");

    let (ok, stdout, stderr) = run(&[
        "queue",
        "add",
        "https://example.com/sample.bin",
        "--server",
        &server,
        "--token",
        &token,
        "--json",
    ]);
    assert!(ok, "queue add failed: {stderr}");
    let created: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let id = created[0]["id"].as_str().expect("id").to_owned();

    let (ok, stdout, stderr) = run(&["queue", "list", "--server", &server, "--token", &token]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("sample.bin"), "{stdout}");
    assert!(stdout.contains("ID"), "the table has a header: {stdout}");

    // Pause, then remove, both addressed by the id the add command printed.
    let (ok, stdout, stderr) = run(&[
        "queue", "pause", &id, "--server", &server, "--token", &token,
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("affected"), "{stdout}");

    // Removing deletes partial data, so it needs the intent spelled out on the command line.
    let (refused, _, stderr, code) = run_with_code(&[
        "queue", "remove", &id, "--server", &server, "--token", &token,
    ]);
    assert!(!refused, "removal without --yes must be refused");
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");

    let (ok, _, stderr) = run(&[
        "queue", "remove", &id, "--yes", "--server", &server, "--token", &token,
    ]);
    assert!(ok, "{stderr}");
    let (_, stdout, _) = run(&["queue", "list", "--server", &server, "--token", &token]);
    assert!(stdout.contains("nothing to show"), "{stdout}");

    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn links_are_handed_over_listed_and_discarded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (server, shutdown, token) = serve(directory.path()).await;

    let (ok, stdout, stderr) = run(&[
        "links",
        "add",
        "https://example.com/one.bin",
        "https://example.com/two.bin",
        "--server",
        &server,
        "--token",
        &token,
    ]);
    assert!(ok, "links add failed: {stderr}");
    assert!(stdout.contains("link(s)"), "{stdout}");

    let (ok, stdout, stderr) = run(&["links", "list", "--server", &server, "--token", &token]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("PACKAGE"), "{stdout}");

    shutdown.cancel();
}

/// Creates a category over the API, with the storage root it needs, and returns its id.
async fn category(server: &str, token: &str, directory: &std::path::Path, name: &str) -> String {
    let http = reqwest::Client::new();
    let path = directory.join("downloads");
    std::fs::create_dir_all(&path).expect("downloads");
    let root: serde_json::Value = http
        .post(format!("{server}/api/v1/storage-roots"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "name": "Primary", "path": path.to_string_lossy(), "is_default": true
        }))
        .send()
        .await
        .expect("storage root")
        .json()
        .await
        .expect("storage root json");
    let created: serde_json::Value = http
        .post(format!("{server}/api/v1/categories"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "name": name, "color": "#38BDF8", "storage_root_id": root["id"],
            "relative_path": "", "is_default": false
        }))
        .send()
        .await
        .expect("category")
        .json()
        .await
        .expect("category json");
    created["id"].as_str().expect("category id").to_owned()
}

async fn get(server: &str, token: &str, path: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{server}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json")
}

/// RD-130-19: `links add --category … --enqueue` puts links straight into the queue with the
/// category, and without `--enqueue` into the LinkGrabber with the category and the package.
#[tokio::test(flavor = "multi_thread")]
async fn links_go_straight_to_the_queue_or_the_linkgrabber_with_a_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (server, shutdown, token) = serve(directory.path()).await;
    let series = category(&server, &token, directory.path(), "Series").await;

    // By name, ignoring case, as somebody types it into a cron line.
    let (ok, stdout, stderr) = run(&[
        "links",
        "add",
        "https://example.com/one.bin",
        "https://example.com/two.bin",
        "--category",
        "series",
        "--package",
        "Nightly",
        "--enqueue",
        "--server",
        &server,
        "--token",
        &token,
        "--json",
    ]);
    assert!(ok, "links add --enqueue failed: {stderr}");
    let created: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(created.as_array().map(Vec::len), Some(2), "{stdout}");
    let downloads = get(&server, &token, "/api/v1/downloads").await;
    assert_eq!(downloads.as_array().map(Vec::len), Some(2), "{downloads}");
    let packages = get(&server, &token, "/api/v1/packages").await;
    let nightly = packages
        .as_array()
        .and_then(|packages| packages.iter().find(|package| package["name"] == "Nightly"))
        .unwrap_or_else(|| panic!("no Nightly package: {packages}"));
    assert_eq!(
        nightly["category_id"],
        serde_json::json!(series),
        "{nightly}"
    );
    // Nothing went through the LinkGrabber.
    let waiting = get(&server, &token, "/api/v1/collector/packages").await;
    assert_eq!(waiting.as_array().map(Vec::len), Some(0), "{waiting}");

    // Without --enqueue: the LinkGrabber, with the category on the package it made.
    let (ok, _, stderr) = run(&[
        "links",
        "add",
        "https://example.com/three.bin",
        "--category",
        &series,
        "--package",
        "Review me",
        "--server",
        &server,
        "--token",
        &token,
    ]);
    assert!(ok, "links add --category failed: {stderr}");
    let waiting = get(&server, &token, "/api/v1/collector/packages").await;
    let review = waiting
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"] == "Review me")
        })
        .unwrap_or_else(|| panic!("no LinkGrabber package: {waiting}"));
    assert_eq!(review["category_id"], serde_json::json!(series), "{review}");

    // A category that does not exist is a usage error, before anything is added.
    let (ok, _, stderr, code) = run_with_code(&[
        "links",
        "add",
        "https://example.com/four.bin",
        "--category",
        "Nope",
        "--enqueue",
        "--server",
        &server,
        "--token",
        &token,
    ]);
    assert!(!ok);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("Nope"), "{stderr}");
    let downloads = get(&server, &token, "/api/v1/downloads").await;
    assert_eq!(downloads.as_array().map(Vec::len), Some(2), "{downloads}");

    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_reports_the_server_s_own_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (server, shutdown, _) = serve(directory.path()).await;

    // The message has to name what went wrong, not just that something did: a script that
    // gets "request failed" cannot tell a bad token from an unreachable host.
    let (ok, _, stderr) = run(&[
        "queue",
        "list",
        "--server",
        &server,
        "--token",
        "not-a-token",
    ]);
    assert!(!ok, "a wrong token must fail");
    assert!(
        stderr.contains("401") || stderr.to_lowercase().contains("login"),
        "{stderr}"
    );

    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_fails_with_its_address() {
    // Port 1 is reserved and nothing listens on it; the message must name the address so
    // the reader can see they pointed at the wrong host.
    let (ok, _, stderr, code) = run_with_code(&["queue", "list", "--server", "http://127.0.0.1:1"]);
    assert!(!ok);
    assert_eq!(
        code, 3,
        "an unreachable host has its own exit code: {stderr}"
    );
    assert!(stderr.contains("127.0.0.1:1"), "{stderr}");
}

#[test]
fn a_server_address_without_a_scheme_is_refused_before_any_request() {
    let (ok, _, stderr, code) = run_with_code(&["queue", "list", "--server", "127.0.0.1:8710"]);
    assert!(!ok);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("http://"), "{stderr}");
}

//! Integration tests for the `/mcp` endpoint: auth gating and the MCP
//! initialize / tools round-trip over streamable HTTP.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const API_BEARER: &str = "test-api-bearer-token";
const CAPTURE_BEARER: &str = "test-capture-bearer-token";
/// Holds queue control and nothing else, so it may drive the queue tools and no others.
const QUEUE_BEARER: &str = "test-queue-bearer-token";
/// Holds configuration and nothing else: categories yes, accounts and plugins no.
const CONFIG_BEARER: &str = "test-config-bearer-token";

async fn test_router(directory: &std::path::Path) -> Router {
    test_parts(directory).await.0
}

/// The same router, with the two stores the canary has to seed through.
///
/// Split out rather than duplicated: a second builder would be a second set of decisions about
/// what the test installation contains, and the canary's whole point is that it runs against
/// the installation the other tests run against.
async fn test_parts(
    directory: &std::path::Path,
) -> (Router, rd_db::Database, rd_secrets::SecretStore) {
    let database = rd_db::Database::open(directory.join("mcp-test.sqlite3"))
        .await
        .expect("database");
    for (bearer, scope) in [
        (API_BEARER, rd_core::API_SCOPE),
        (CAPTURE_BEARER, rd_core::CAPTURE_SCOPE),
        (QUEUE_BEARER, rd_core::API_QUEUE_SCOPE),
        (CONFIG_BEARER, rd_core::API_CONFIG_SCOPE),
    ] {
        database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                scope.to_owned(),
                hex::encode(Sha256::digest(bearer.as_bytes())),
                vec![scope.to_owned()],
            )
            .await
            .expect("token");
    }
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let plugins = rd_plugin_host::PluginInstaller::new(
        directory.join("plugins"),
        rd_plugin_host::PluginVerifier::new(true),
    );
    let media_settings = rd_media::shared_settings(&database)
        .await
        .expect("media settings");
    let (_media_runner, media_probe) =
        rd_media::build(database.clone(), secrets.clone(), media_settings.clone());
    let gallery_settings = rd_gallery::shared_settings(&database)
        .await
        .expect("gallery settings");
    let stream_settings = rd_stream::shared_settings(&database)
        .await
        .expect("stream settings");
    let torrent_settings = rd_torrent::shared_settings(&database)
        .await
        .expect("torrent settings");
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
    let database_handle = database.clone();
    let secrets_handle = secrets.clone();
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
    (rd_api::router(state), database_handle, secrets_handle)
}

fn mcp_request(bearer: Option<&str>, session: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body.to_owned())).expect("request")
}

/// Extracts the JSON-RPC payload from a JSON or SSE response body.
fn extract_json(content_type: &str, body: &str) -> serde_json::Value {
    let payload = if content_type.starts_with("text/event-stream") {
        body.lines()
            .rev()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("SSE data line")
            .to_owned()
    } else {
        body.to_owned()
    };
    serde_json::from_str(&payload).expect("JSON-RPC payload")
}

async fn call(router: &Router, request: Request<Body>) -> (StatusCode, String, String) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"mcp-test","version":"0.0.0"}}}"#;

#[tokio::test]
async fn mcp_endpoint_requires_a_token_with_an_api_scope() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, _, _) = call(&router, mcp_request(None, None, INITIALIZE)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A capture-scoped token must not unlock the MCP endpoint. It carries a scope, so a
    // check that merely counted them would let it through; the endpoint asks for an *API*
    // scope specifically.
    let (status, _, _) = call(&router, mcp_request(Some(CAPTURE_BEARER), None, INITIALIZE)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // An invented token is rejected as well.
    let (status, _, _) = call(&router, mcp_request(Some("nonsense"), None, INITIALIZE)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The refusal says what kind of credential would have worked.
///
/// Without this header a client learns only that something was wrong, which is how a
/// misconfigured connector ends up looking like a broken server. The MCP specification has
/// clients read `WWW-Authenticate` for exactly this, so the bare `401` this used to send was
/// both unhelpful and out of contract.
#[tokio::test]
async fn the_refusal_carries_a_bearer_challenge() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let response = router
        .clone()
        .oneshot(mcp_request(None, None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .expect("a WWW-Authenticate header");
    assert!(
        challenge.starts_with("Bearer"),
        "the challenge must name the bearer scheme, got {challenge}"
    );
    // No `resource_metadata`: this service runs no authorization server, and advertising one
    // would send an OAuth-capable client into a flow that cannot complete.
    assert!(
        !challenge.contains("resource_metadata"),
        "no OAuth metadata may be advertised, got {challenge}"
    );
}

#[tokio::test]
async fn mcp_initialize_and_tool_calls_round_trip() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    // Initialize handshake establishes a session.
    let response = router
        .clone()
        .oneshot(mcp_request(Some(API_BEARER), None, INITIALIZE))
        .await
        .expect("response");
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        panic!(
            "initialize failed: {status} {}",
            String::from_utf8_lossy(&body)
        );
    }
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session id")
        .to_owned();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let init = extract_json(&content_type, &String::from_utf8_lossy(&body));
    assert!(init["result"]["serverInfo"]["name"].is_string());
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // The client acknowledges the handshake.
    let ack = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let (status, _, _) = call(&router, mcp_request(Some(API_BEARER), Some(&session), ack)).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // tools/list advertises the download tools.
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    let response = router
        .clone()
        .oneshot(mcp_request(Some(API_BEARER), Some(&session), list))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let tools = extract_json(&content_type, &String::from_utf8_lossy(&body));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for expected in [
        "add_downloads",
        "list_downloads",
        "get_download",
        "control_downloads",
        "get_status_summary",
        "list_packages",
        "delete_packages",
        "collect_links",
        "check_links",
        "list_collector",
        "enqueue_collector",
        "get_settings",
        "update_settings",
        "list_configuration",
        "create_category",
        "update_category",
        "delete_category",
        "create_category_rule",
        "create_storage_root",
        "create_hotfolder",
        "create_account",
        "update_proxy_profile",
        "delete_proxy_profile",
        "create_usenet_server",
        "create_notification_target",
        "create_notification_rule",
        "create_subscription",
        "create_stream_channel",
        "create_automation",
        "set_plugin_enabled",
        "uninstall_plugin_version",
    ] {
        assert!(names.contains(&expected), "missing tool: {expected}");
    }

    // A real tool call: the empty queue summary.
    let summary = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_status_summary","arguments":{}}}"#;
    let response = router
        .clone()
        .oneshot(mcp_request(Some(API_BEARER), Some(&session), summary))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let result = extract_json(&content_type, &String::from_utf8_lossy(&body));
    assert_ne!(result["result"]["isError"], true);
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("summary text");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("summary JSON");
    assert_eq!(parsed["queued"], 0);
    assert_eq!(parsed["active"], 0);

    // Round-trip: add a paused download, see it listed, remove it again.
    let tool_call = |id: u32, name: &str, arguments: serde_json::Value| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        })
        .to_string()
    };
    let tool_text = |value: &serde_json::Value| -> serde_json::Value {
        assert_ne!(value["result"]["isError"], true, "tool errored: {value}");
        serde_json::from_str(
            value["result"]["content"][0]["text"]
                .as_str()
                .expect("text"),
        )
        .expect("tool JSON")
    };

    let add = tool_call(
        4,
        "add_downloads",
        serde_json::json!({
            "urls": ["http://127.0.0.1:9/unreachable.bin"],
            "package_name": "MCP Test",
            "start_paused": true
        }),
    );
    let (status, ct, body) =
        call(&router, mcp_request(Some(API_BEARER), Some(&session), &add)).await;
    assert_eq!(status, StatusCode::OK);
    let added = tool_text(&extract_json(&ct, &body));
    assert_eq!(added["created"].as_array().expect("created").len(), 1);
    assert_eq!(added["failed"].as_array().expect("failed").len(), 0);
    let download_id = added["created"][0]["download"]["id"]
        .as_str()
        .expect("download id")
        .to_owned();

    let list = tool_call(5, "list_downloads", serde_json::json!({}));
    let (status, ct, body) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &list),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = tool_text(&extract_json(&ct, &body));
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["items"][0]["file_name"], "unreachable.bin");

    let remove = tool_call(
        6,
        "control_downloads",
        serde_json::json!({ "action": "remove", "ids": [download_id] }),
    );
    let (status, ct, body) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &remove),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let removed = tool_text(&extract_json(&ct, &body));
    assert_eq!(removed["affected"], 1);

    let (status, ct, body) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &list),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = tool_text(&extract_json(&ct, &body));
    assert_eq!(listed["total"], 0);
}

/// A scoped token drives the tools its scope covers and is refused by the rest.
///
/// The sixteen tools used to sit behind one blanket `api:*` check, so letting an assistant see
/// the queue meant letting it read every stored account and rewrite the settings document.
/// This walks both sides of that with one token: `list_downloads` is reading, which queue
/// control confers, and `list_configuration` is credentials, which nothing confers.
#[tokio::test]
async fn a_scoped_token_reaches_only_the_tools_its_scope_covers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let response = router
        .clone()
        .oneshot(mcp_request(Some(QUEUE_BEARER), None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "initialize");
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session id")
        .to_owned();
    let ack = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let (status, _, _) = call(
        &router,
        mcp_request(Some(QUEUE_BEARER), Some(&session), ack),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // Queue control confers reading, so this one goes through.
    let allowed = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_downloads","arguments":{}}}"#;
    let (status, content_type, body) = call(
        &router,
        mcp_request(Some(QUEUE_BEARER), Some(&session), allowed),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let answer = extract_json(&content_type, &body);
    assert!(
        answer["error"].is_null(),
        "list_downloads was refused for a queue token: {answer}"
    );

    // Configuration is conferred by nothing, queue control included.
    let refused = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_configuration","arguments":{"section":"categories"}}}"#;
    let (status, content_type, body) = call(
        &router,
        mcp_request(Some(QUEUE_BEARER), Some(&session), refused),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "JSON-RPC reports errors in the body"
    );
    let answer = extract_json(&content_type, &body);
    let message = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("api:config"),
        "list_configuration did not name the permission it wanted: {answer}"
    );
    // The refusal carries the same stable code the REST layer returns, so a client can react
    // to it without reading English prose.
    assert_eq!(answer["error"]["data"]["code"], "auth.scope_insufficient");
    assert_eq!(answer["error"]["data"]["scope"], "api:config");

    // A writing tool is refused just as a reading one is; the queue token holds no config.
    let write = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"delete_category","arguments":{"id":"whatever"}}}"#;
    let (_, content_type, body) = call(
        &router,
        mcp_request(Some(QUEUE_BEARER), Some(&session), write),
    )
    .await;
    let answer = extract_json(&content_type, &body);
    assert_eq!(answer["error"]["data"]["code"], "auth.scope_insufficient");
    assert_eq!(answer["error"]["data"]["scope"], "api:config");
}

/// One tool, three prices: `list_configuration` costs what the section it is asked for costs.
///
/// A configuration token exists to manage categories and routing. It must not thereby become a
/// way to enumerate every stored account, nor to read the plugin inventory — `api:secrets` and
/// `api:admin` are separate grants, and neither is implied by `api:config`.
#[tokio::test]
async fn a_configuration_token_reads_categories_but_not_accounts_or_plugins() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, CONFIG_BEARER).await;

    let allowed = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_configuration","arguments":{"section":"categories"}}}"#;
    let (_, content_type, body) = call(
        &router,
        mcp_request(Some(CONFIG_BEARER), Some(&session), allowed),
    )
    .await;
    let answer = extract_json(&content_type, &body);
    assert!(
        answer["error"].is_null(),
        "the categories section was refused for a config token: {answer}"
    );

    for (id, section, scope) in [
        (3, "accounts", "api:secrets"),
        (4, "proxy_profiles", "api:secrets"),
        (5, "plugins", "api:admin"),
    ] {
        let refused = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call",
            "params":{{"name":"list_configuration","arguments":{{"section":"{section}"}}}}}}"#
        );
        let (_, content_type, body) = call(
            &router,
            mcp_request(Some(CONFIG_BEARER), Some(&session), &refused),
        )
        .await;
        let answer = extract_json(&content_type, &body);
        assert_eq!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{section} was not refused: {answer}"
        );
        assert_eq!(answer["error"]["data"]["scope"], scope, "{section}");
    }
}

/// The reported gap, end to end: create a category, change it, list it, remove it again.
#[tokio::test]
async fn a_category_is_created_changed_and_removed_over_mcp() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let root = tool_result(
        &router,
        &session,
        2,
        "create_storage_root",
        serde_json::json!({
            "name": "Downloads",
            "path": directory.path().join("roots/main").to_string_lossy(),
        }),
    )
    .await;
    let root_id = root["id"].as_str().expect("root id").to_owned();

    let created = tool_result(
        &router,
        &session,
        3,
        "create_category",
        serde_json::json!({
            "name": "Movies",
            "color": "#112233",
            "storage_root_id": root_id,
            "relative_path": "movies",
        }),
    )
    .await;
    let category_id = created["id"].as_str().expect("category id").to_owned();
    assert_eq!(created["name"], "Movies");

    // A partial change keeps everything it does not mention — the reason update tools merge.
    let changed = tool_result(
        &router,
        &session,
        4,
        "update_category",
        serde_json::json!({ "id": category_id, "color": "#445566" }),
    )
    .await;
    assert_eq!(changed["color"], "#445566");
    assert_eq!(changed["name"], "Movies");
    assert_eq!(changed["relative_path"], "movies");

    let listed = tool_result(
        &router,
        &session,
        5,
        "list_configuration",
        serde_json::json!({ "section": "categories" }),
    )
    .await;
    assert_eq!(listed.as_array().expect("categories").len(), 1);

    tool_result(
        &router,
        &session,
        6,
        "delete_category",
        serde_json::json!({ "id": category_id }),
    )
    .await;
    let listed = tool_result(
        &router,
        &session,
        7,
        "list_configuration",
        serde_json::json!({ "section": "categories" }),
    )
    .await;
    assert!(listed.as_array().expect("categories").is_empty());
}

/// A credential-bearing row is created, changed and removed without a secret ever appearing.
///
/// A proxy profile rather than an account, because `create_account` needs a provider from the
/// registry and the registry is filled from installed plugins, which this router has none of.
/// The response side of an account is settled by its type instead: `rd_core::Account` carries
/// `has_secret` and `has_cookies`, never the values or their vault references.
#[tokio::test]
async fn a_credential_bearing_row_round_trips_without_a_secret() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let proxy = tool_result(
        &router,
        &session,
        2,
        "create_proxy_profile",
        serde_json::json!({
            "name": "House proxy",
            "kind": "socks5",
            "endpoint": "socks5://127.0.0.1:1080",
        }),
    )
    .await;
    let proxy_id = proxy["id"].as_str().expect("proxy id").to_owned();
    assert!(proxy.get("secret_ref").is_none());
    assert_eq!(proxy["has_credentials"], false);

    let renamed = tool_result(
        &router,
        &session,
        3,
        "update_proxy_profile",
        serde_json::json!({ "id": proxy_id, "name": "Attic proxy" }),
    )
    .await;
    assert_eq!(renamed["name"], "Attic proxy");
    assert_eq!(renamed["endpoint"], "socks5://127.0.0.1:1080");

    tool_result(
        &router,
        &session,
        4,
        "delete_proxy_profile",
        serde_json::json!({ "id": proxy_id }),
    )
    .await;
}

/// A free-form body may not smuggle a credential past the "no secrets here" rule.
///
/// `no_tool_accepts_a_credential` walks the published input schemas, which settles every tool
/// that names its fields. The two shapes it cannot settle are the ones whose schema is an open
/// object: the `definition` passthroughs, and a notification destination's `config`, which is
/// stored and read back verbatim and would therefore hand the value straight back out again.
#[tokio::test]
async fn a_definition_carrying_an_api_key_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let smuggled = [
        (
            2,
            "create_subscription",
            serde_json::json!({
                "definition": {
                    "name": "Indexer",
                    "url": "https://indexer.example/api",
                    "kind": "newznab",
                    "api_key": "not-through-here",
                }
            }),
        ),
        (
            3,
            "create_notification_target",
            serde_json::json!({
                "name": "Mailer",
                "kind": "smtp",
                "endpoint": "smtp.example:587",
                "config": { "from": "rd@example", "password": "not-through-here" },
            }),
        ),
    ];

    for (id, name, arguments) in smuggled {
        let call_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        })
        .to_string();
        let (_, content_type, body) = call(
            &router,
            mcp_request(Some(API_BEARER), Some(&session), &call_body),
        )
        .await;
        let answer = extract_json(&content_type, &body);
        assert_eq!(answer["result"]["isError"], true, "{name}: {answer}");
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .expect("error text");
        let parsed: serde_json::Value = serde_json::from_str(text).expect("error JSON");
        assert_eq!(parsed["code"], "request.credential_rejected", "{name}");
    }
}

/// Completes the MCP handshake and returns the session id.
async fn handshake(router: &Router, bearer: &str) -> String {
    let response = router
        .clone()
        .oneshot(mcp_request(Some(bearer), None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "initialize");
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session id")
        .to_owned();
    let ack = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let (status, _, _) = call(router, mcp_request(Some(bearer), Some(&session), ack)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    session
}

/// Calls one tool with the full-access token and returns its parsed success payload.
async fn tool_result(
    router: &Router,
    session: &str,
    id: u32,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
    .to_string();
    let (status, content_type, response) =
        call(router, mcp_request(Some(API_BEARER), Some(session), &body)).await;
    assert_eq!(status, StatusCode::OK, "{name}");
    let answer = extract_json(&content_type, &response);
    assert!(answer["error"].is_null(), "{name} was refused: {answer}");
    assert_ne!(answer["result"]["isError"], true, "{name} failed: {answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
}

/// The tools RD-120-29 added cost exactly what their routes cost, and refuse without it.
///
/// A configuration token is the sharpest instrument for this: site rules are `api:config`, so
/// it drives them; remote jobs are `api:secrets` because they name an account, and the log and
/// audit stores are `api:admin`. All three refusals come from one table read rather than from
/// a check written into each tool, so this is the proof that the table is actually consulted
/// for the new entries too — a tool with no entry would fail closed with a different code, and
/// a tool priced from the wrong route would pass here while being cheaper than its endpoint.
#[tokio::test]
async fn the_new_tools_cost_what_their_routes_cost() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, CONFIG_BEARER).await;

    let allowed = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_site_rules","arguments":{}}}"#;
    let (_, content_type, body) = call(
        &router,
        mcp_request(Some(CONFIG_BEARER), Some(&session), allowed),
    )
    .await;
    let answer = extract_json(&content_type, &body);
    assert!(
        answer["error"].is_null(),
        "list_site_rules was refused for a config token: {answer}"
    );

    for (id, tool, arguments, scope) in [
        (3, "list_remote_jobs", "{}", "api:secrets"),
        (
            4,
            "forget_remote_job",
            r#"{"id":"whatever"}"#,
            "api:secrets",
        ),
        (
            5,
            "submit_remote_job",
            r#"{"account_id":"whatever","magnet":"magnet:?xt=urn:btih:0"}"#,
            "api:secrets",
        ),
        (
            6,
            "choose_remote_job_entries",
            r#"{"id":"whatever","entries":[1]}"#,
            "api:secrets",
        ),
        (7, "list_log_records", "{}", "api:admin"),
        (8, "list_audit_records", "{}", "api:admin"),
    ] {
        let refused = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call",
            "params":{{"name":"{tool}","arguments":{arguments}}}}}"#
        );
        let (_, content_type, body) = call(
            &router,
            mcp_request(Some(CONFIG_BEARER), Some(&session), &refused),
        )
        .await;
        let answer = extract_json(&content_type, &body);
        assert_eq!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{tool} was not refused for a config token: {answer}"
        );
        assert_eq!(
            answer["error"]["data"]["scope"], scope,
            "{tool} named the wrong permission: {answer}"
        );
    }

    // And the refusal happens before the work, not after it: an id the installation does not
    // have would otherwise answer "not found" and tell a caller without the permission that
    // the row is absent.
    let session = handshake(&router, API_BEARER).await;
    let answer = tool_result(
        &router,
        &session,
        9,
        "list_remote_jobs",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(answer.as_array().expect("a list").len(), 0);
}

/// No tool hands back a stored credential, whatever it is asked.
///
/// The canary, after the pattern of `plugins/mega/tests/key_canary.rs`. "No tool returns a
/// secret" is an acceptance criterion, and an acceptance criterion resting on everybody
/// remembering not to serialise a field is not one. `mcp::tests::no_tool_accepts_a_credential`
/// already walks every input schema; this walks the answers. One secret is put in the vault
/// and referenced from the three rows that hold credentials, and every tool that reads without
/// needing an id is then called and its whole answer searched — including `list_log_records`
/// and `list_audit_records`, which are new and are the two that read back what the service
/// itself wrote while those rows were being created.
#[tokio::test]
async fn no_tool_answers_with_a_stored_credential() {
    /// Nothing else in the installation can produce this string, so a hit is a leak and never
    /// a coincidence.
    const CANARY: &str = "rd-120-29-canary-9d41ba7c0e5f4a2b";

    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database, secrets) = test_parts(directory.path()).await;

    let reference = secrets
        .put_string(CANARY.to_owned())
        .await
        .expect("the vault takes the canary");
    // The reference is not the secret, so a row may carry it; the value behind it may not come
    // back out. Asserting that here keeps the test honest about what it is looking for.
    assert!(!reference.contains(CANARY));

    database
        .create_account(rd_db::NewAccount {
            provider: "ddownload".to_owned(),
            label: "canary account".to_owned(),
            username: Some("canary-user".to_owned()),
            credential_mode: None,
            secret_ref: Some(reference.clone()),
            cookie_ref: Some(reference.clone()),
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    database
        .create_usenet_server(rd_db::NewUsenetServer {
            name: "canary news".to_owned(),
            host: "news.invalid".to_owned(),
            port: 563,
            tls: true,
            username: Some("canary-user".to_owned()),
            password_ref: Some(reference.clone()),
            proxy_profile_id: None,
            priority: 1,
            max_connections: 1,
            enabled: false,
        })
        .await
        .expect("usenet server");
    database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "canary proxy".to_owned(),
            kind: rd_core::ProxyKind::Http,
            endpoint: "http://127.0.0.1:9/".parse().expect("endpoint"),
            username: Some("canary-user".to_owned()),
            secret_ref: Some(reference.clone()),
        })
        .await
        .expect("proxy profile");

    let session = handshake(&router, API_BEARER).await;
    let reads = id_free_reads();

    let mut seen_the_rows = false;
    for (index, (tool, arguments)) in reads.iter().enumerate() {
        let answer = tool_result(
            &router,
            &session,
            100 + index as u32,
            tool,
            arguments.clone(),
        )
        .await;
        let rendered = serde_json::to_string(&answer).expect("the answer serialises");
        assert!(
            !rendered.contains(CANARY),
            "{tool} handed back the stored credential: {rendered}"
        );
        // The canary would be worthless if the tools answered with nothing at all, so at least
        // one answer has to contain the rows it was seeded into.
        if rendered.contains("canary-user") {
            seen_the_rows = true;
        }
    }
    assert!(
        seen_the_rows,
        "no tool returned the seeded rows, so nothing was actually searched"
    );
}

/// Every tool that reads and needs no id of its own. The six `list_configuration` sections
/// are named individually because each reads a different store, and `accounts`,
/// `proxy_profiles` and `plugins` are the three that touch what was just seeded.
/// Shared with RD-120-57's indexer-key test, which calls them with a key in the LinkGrabber.
fn id_free_reads() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("list_downloads", serde_json::json!({})),
        ("get_status_summary", serde_json::json!({})),
        ("list_packages", serde_json::json!({})),
        ("list_collector", serde_json::json!({})),
        ("get_settings", serde_json::json!({})),
        ("get_about", serde_json::json!({})),
        (
            "list_configuration",
            serde_json::json!({ "section": "accounts" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "proxy_profiles" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "categories" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "storage_roots" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "providers" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "plugins" }),
        ),
        ("list_category_rules", serde_json::json!({})),
        ("list_hotfolders", serde_json::json!({})),
        ("list_automations", serde_json::json!({})),
        ("list_notification_rules", serde_json::json!({})),
        ("list_notification_targets", serde_json::json!({})),
        ("list_subscriptions", serde_json::json!({})),
        ("list_stream_channels", serde_json::json!({})),
        ("list_usenet_servers", serde_json::json!({})),
        ("list_remote_jobs", serde_json::json!({})),
        ("list_site_rules", serde_json::json!({})),
        ("get_transfer_stats", serde_json::json!({})),
        ("list_log_records", serde_json::json!({ "limit": 500 })),
        ("list_audit_records", serde_json::json!({ "limit": 500 })),
        // RD-120-32's reads that need no id. The ones that do are searched in `everything`.
        ("list_candidates", serde_json::json!({})),
        ("get_mirror_preference", serde_json::json!({})),
        ("list_nzb_imports", serde_json::json!({})),
        (
            "list_postprocess_options",
            serde_json::json!({ "kind": "scripts" }),
        ),
        (
            "list_postprocess_options",
            serde_json::json!({ "kind": "plugin_steps" }),
        ),
        (
            "list_postprocess_options",
            serde_json::json!({ "kind": "upload_destinations" }),
        ),
        ("list_postprocess_queue", serde_json::json!({})),
        ("list_managed_tools", serde_json::json!({ "view": "tools" })),
        ("list_managed_tools", serde_json::json!({ "view": "media" })),
        ("get_storage_capacity", serde_json::json!({})),
        (
            "get_torrent_engine",
            serde_json::json!({ "view": "capabilities" }),
        ),
        (
            "get_torrent_engine",
            serde_json::json!({ "view": "network_status" }),
        ),
        ("list_network_interfaces", serde_json::json!({})),
    ]
}

/// The plugin inventory a tool answers with is the one the route answers with.
///
/// RD-120-28 found this while adding `execution_count` to `GET /api/v1/plugins`: the tool
/// built its answer from the manifests alone, which cannot see the execution store, so it
/// reported `active: false` for every plugin and would have reported `execution_count: 0` for
/// one that had run a thousand times. A model reading `0` concludes the plugin never ran, so a
/// wrong number here is worse than a missing one.
///
/// Pinned by shape rather than by value because a fresh installation has no plugins: the route
/// answers with an inventory — `installed` and `incompatible` — and the manifest list it used
/// to build was a bare array that silently dropped the incompatible half. If someone
/// reintroduces the second implementation, this fails.
#[tokio::test]
async fn the_plugin_section_answers_with_the_route_s_inventory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let answer = tool_result(
        &router,
        &session,
        2,
        "list_configuration",
        serde_json::json!({ "section": "plugins" }),
    )
    .await;
    assert!(
        answer
            .get("installed")
            .is_some_and(serde_json::Value::is_array),
        "the plugins section is not the route's inventory: {answer}"
    );
    assert!(
        answer
            .get("incompatible")
            .is_some_and(serde_json::Value::is_array),
        "the plugins section dropped the incompatible half: {answer}"
    );
}

/// RD-120-37: the view and autoplay settings travel through the MCP definition passthrough.
#[tokio::test]
async fn a_subscription_view_is_set_over_mcp() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let definition = serde_json::json!({
        "name": "Music",
        "url": "https://example.test/feed.xml",
        "kind": "feed",
        "interval_seconds": 3_600,
    });
    let created = tool_result(
        &router,
        &session,
        2,
        "create_subscription",
        serde_json::json!({ "definition": definition }),
    )
    .await;
    assert_eq!(created["view"], "list");
    assert_eq!(created["autoplay"], false);
    let id = created["id"].as_str().expect("id").to_owned();

    let mut changed = definition.clone();
    changed["view"] = serde_json::json!("cards");
    changed["autoplay"] = serde_json::json!(true);
    let updated = tool_result(
        &router,
        &session,
        3,
        "update_subscription",
        serde_json::json!({ "id": id, "definition": changed }),
    )
    .await;
    assert_eq!(updated["view"], "cards");
    assert_eq!(updated["autoplay"], true);
}

/// A container is handed in over MCP (RD-120-31), at the price of the route underneath.
///
/// A queue token holds everything a person would call "working the queue" and not intake, so
/// it is refused all three tools under `api:intake` — the permission read from the import
/// routes' own `scope_policy` entries. The full token then hands in a recorded torrent, a
/// recorded NZB and a link list, and a broken body comes back with the route's own code.
#[tokio::test]
async fn a_container_is_handed_in_over_mcp_at_the_price_of_its_route() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    const TORRENT: &[u8] = include_bytes!("../../../testfile/big-buck-bunny.torrent");
    const NZB: &[u8] = include_bytes!("../../../testfile/sabnzbd-test-download-100MB.nzb");

    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, QUEUE_BEARER).await;
    for (id, tool) in [
        (2, "import_container"),
        (3, "import_torrent"),
        (4, "import_nzb"),
    ] {
        let refused = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": { "content": "aGVsbG8=" } }
        })
        .to_string();
        let (_, content_type, body) = call(
            &router,
            mcp_request(Some(QUEUE_BEARER), Some(&session), &refused),
        )
        .await;
        let answer = extract_json(&content_type, &body);
        assert_eq!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{tool}: {answer}"
        );
        assert_eq!(answer["error"]["data"]["scope"], "api:intake", "{tool}");
    }

    let session = handshake(&router, API_BEARER).await;
    let torrent = tool_result(
        &router,
        &session,
        5,
        "import_torrent",
        serde_json::json!({ "file_name": "bbb.torrent", "content": STANDARD.encode(TORRENT) }),
    )
    .await;
    assert_eq!(
        torrent["candidates"].as_array().map(Vec::len),
        Some(1),
        "{torrent}"
    );
    let nzb = tool_result(
        &router,
        &session,
        6,
        "import_nzb",
        serde_json::json!({ "file_name": "test.nzb", "content": STANDARD.encode(NZB) }),
    )
    .await;
    assert_eq!(nzb["sha256"], hex::encode(Sha256::digest(NZB)), "{nzb}");
    let list = tool_result(
        &router,
        &session,
        7,
        "import_container",
        serde_json::json!({
            "file_name": "links.txt",
            "content": STANDARD.encode("https://example.invalid/a.bin\n"),
        }),
    )
    .await;
    assert_eq!(list["format"], "text", "{list}");

    let broken = serde_json::json!({
        "jsonrpc": "2.0", "id": 8, "method": "tools/call",
        "params": { "name": "import_container", "arguments": { "content": "not base64!" } }
    })
    .to_string();
    let (_, content_type, body) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &broken),
    )
    .await;
    let answer = extract_json(&content_type, &body);
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("error text");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("error JSON");
    assert_eq!(parsed["code"], "container.base64_invalid");
}

/// RD-120-42: the card ratio travels through the MCP definition passthrough, and an unknown
/// one is refused there with the same stable code as over REST.
#[tokio::test]
async fn a_card_ratio_is_set_over_mcp_and_an_unknown_one_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let definition = serde_json::json!({
        "name": "Covers",
        "url": "https://example.test/feed.xml",
        "kind": "feed",
        "interval_seconds": 3_600,
    });
    let created = tool_result(
        &router,
        &session,
        2,
        "create_subscription",
        serde_json::json!({ "definition": definition }),
    )
    .await;
    assert_eq!(created["card_ratio"], "2:1");
    let id = created["id"].as_str().expect("id").to_owned();

    let mut changed = definition.clone();
    changed["view"] = serde_json::json!("cards");
    changed["card_ratio"] = serde_json::json!("1:1");
    let updated = tool_result(
        &router,
        &session,
        3,
        "update_subscription",
        serde_json::json!({ "id": id, "definition": changed }),
    )
    .await;
    assert_eq!(updated["card_ratio"], "1:1");

    let mut unknown = definition.clone();
    unknown["card_ratio"] = serde_json::json!("21:9");
    let call_body = serde_json::json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": { "name": "create_subscription", "arguments": { "definition": unknown } }
    })
    .to_string();
    let (_, content_type, body) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &call_body),
    )
    .await;
    let answer = extract_json(&content_type, &body);
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("error text");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("error JSON");
    assert_eq!(parsed["code"], "subscription.card_ratio_unknown");
}

/// Calls a tool that must fail, and returns the error body it failed with.
async fn tool_refusal(
    router: &Router,
    session: &str,
    id: u32,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
    .to_string();
    let (_, content_type, response) =
        call(router, mcp_request(Some(API_BEARER), Some(session), &body)).await;
    let answer = extract_json(&content_type, &response);
    assert_eq!(
        answer["result"]["isError"], true,
        "{name} was not refused: {answer}"
    );
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("error text");
    serde_json::from_str(text).expect("error JSON")
}

/// RD-130-19, the owner's line: no tool starts code on the machine. A script subscription is
/// neither made, changed, switched on or off nor run through MCP -- not even with a token that
/// holds every scope -- while reading it stays open.
#[tokio::test]
async fn no_tool_makes_changes_switches_on_or_runs_a_script_subscription() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let scripts = directory.path().join("scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::write(
        scripts.join("daily-links.sh"),
        "echo https://example.test/a.rar\n",
    )
    .expect("script");
    let session = handshake(&router, API_BEARER).await;
    let definition = serde_json::json!({
        "name": "Daily links",
        "url": "script:daily-links.sh",
        "kind": "script",
        "interval_seconds": 3_600,
        "schedule": "0 6 * * *",
    });

    let refused = tool_refusal(
        &router,
        &session,
        2,
        "create_subscription",
        serde_json::json!({ "definition": definition }),
    )
    .await;
    assert_eq!(refused["code"], "subscription.script_via_mcp", "{refused}");

    // Made by the administrator over REST, where it belongs.
    let (status, created) = common::post_with_bearer(
        &router,
        "/api/v1/subscriptions",
        API_BEARER,
        definition.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();

    for (call_id, name, arguments) in [
        (3, "poll_subscription", serde_json::json!({ "id": id })),
        (
            4,
            "set_subscription_enabled",
            serde_json::json!({ "id": id, "enabled": true }),
        ),
        (
            5,
            "update_subscription",
            serde_json::json!({ "id": id, "definition": definition }),
        ),
        (
            6,
            "set_subscription_enabled",
            serde_json::json!({ "id": id, "enabled": false }),
        ),
    ] {
        let refused = tool_refusal(&router, &session, call_id, name, arguments).await;
        assert_eq!(
            refused["code"], "subscription.script_via_mcp",
            "{name}: {refused}"
        );
    }

    let listed = tool_result(
        &router,
        &session,
        7,
        "list_subscriptions",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(listed[0]["kind"], "script", "{listed}");
    assert_eq!(listed[0]["enabled"], true, "{listed}");
    // Nothing ran: the history is empty.
    let (_, runs) = common::get_with_bearer(
        &router,
        &format!("/api/v1/subscriptions/{id}/runs"),
        API_BEARER,
    )
    .await;
    assert_eq!(runs.as_array().map(Vec::len), Some(0), "{runs}");
}

/// RD-120-32's tools: price, listed ids and the canary, one file of their own.
#[path = "mcp/everything.rs"]
mod everything;

/// Queues a magnet over REST and leaves its torrent in the persisted engine session.
async fn queued_magnet(
    router: &Router,
    directory: &std::path::Path,
    database: &rd_db::Database,
    hash: &str,
) -> (String, String) {
    let request = Request::post("/api/v1/downloads")
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {API_BEARER}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "url": format!("magnet:?xt=urn:btih:{hash}") }).to_string(),
        ))
        .expect("request");
    let (status, _, body) = call(router, request).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    common::persist_torrent_session(directory, hash);
    let download = database.list_downloads().await.expect("downloads")[0].clone();
    (download.id.to_string(), download.package_id.to_string())
}

/// RD-120-68: both MCP removal tools take the torrent out of the engine session.
#[tokio::test]
async fn removing_over_mcp_takes_the_torrent_out_of_the_session() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database, _) = test_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let (download, _) = queued_magnet(
        &router,
        directory.path(),
        &database,
        "1111111111111111111111111111111111111111",
    )
    .await;
    tool_result(
        &router,
        &session,
        2,
        "control_downloads",
        serde_json::json!({ "action": "remove", "ids": [download] }),
    )
    .await;
    assert!(
        database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty()
    );
    assert!(common::persisted_torrents(directory.path()).is_empty());

    let (_, package) = queued_magnet(
        &router,
        directory.path(),
        &database,
        "2222222222222222222222222222222222222222",
    )
    .await;
    tool_result(
        &router,
        &session,
        3,
        "delete_packages",
        serde_json::json!({ "ids": [package], "force": true }),
    )
    .await;
    assert!(
        database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty()
    );
    assert!(common::persisted_torrents(directory.path()).is_empty());
}

/// Calls `update_settings` with `patch` as `bearer`; the parsed tool answer, and whether the
/// tool reported it as an error.
async fn update_settings_as(
    router: &Router,
    bearer: &str,
    session: &str,
    id: u32,
    patch: serde_json::Value,
) -> (bool, serde_json::Value) {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": "update_settings", "arguments": { "patch": patch } }
    })
    .to_string();
    let (status, content_type, response) =
        call(router, mcp_request(Some(bearer), Some(session), &body)).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let answer = extract_json(&content_type, &response);
    assert!(
        answer["error"].is_null(),
        "refused before the tool ran: {answer}"
    );
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    (
        answer["result"]["isError"] == true,
        serde_json::from_str(text).expect("tool JSON"),
    )
}

/// The `settings_changed` audit records, newest first.
async fn settings_audit(database: &rd_db::Database) -> Vec<serde_json::Value> {
    database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 100,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .map(|record| serde_json::to_value(record).expect("json"))
        .filter(|record| record["action"] == "settings_changed")
        .collect()
}

/// `update_settings` costs `api:config`, like `PUT /api/v1/settings` — and like that route it
/// must not let a configuration token change the fields that decide who may reach the service.
///
/// Until RD-130-09 the tool called `apply_settings` directly and skipped the gate the REST route
/// applies, so a config token could switch the administrator login off over MCP and with that
/// hand every caller the full set of scopes. The refusal is audited like the REST one.
#[tokio::test]
async fn a_configuration_token_cannot_change_privileged_settings_over_mcp() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database, _secrets) = test_parts(directory.path()).await;
    let session = handshake(&router, CONFIG_BEARER).await;

    for (id, field, value) in [
        (2, "admin_login_disabled", serde_json::json!(true)),
        (3, "session_max_hours", serde_json::json!(2160)),
        (4, "session_idle_hours", serde_json::json!(720)),
    ] {
        let (failed, answer) = update_settings_as(
            &router,
            CONFIG_BEARER,
            &session,
            id,
            serde_json::json!({ field: value }),
        )
        .await;
        assert!(failed, "{field} was changed by a config token: {answer}");
        assert_eq!(
            answer["code"], "auth.scope_insufficient",
            "{field}: {answer}"
        );
        let refused = settings_audit(&database).await;
        let newest = refused.first().expect("no settings_changed record");
        assert_eq!(newest["outcome"], "failure", "{field}: {newest}");
        assert_eq!(newest["details"]["refused_field"], field, "{newest}");
    }
    assert_ne!(
        database
            .service_setting_field::<bool>("admin_login_disabled")
            .await
            .expect("read"),
        Some(true),
        "the administrator login was switched off anyway"
    );

    // An ordinary field still goes through for the same token, or the gate would have
    // swallowed the scope it is meant to leave alone.
    let (failed, answer) = update_settings_as(
        &router,
        CONFIG_BEARER,
        &session,
        5,
        serde_json::json!({ "max_active_files": 3 }),
    )
    .await;
    assert!(!failed, "{answer}");
    assert_eq!(answer["max_active_files"], 3);
}

/// An administration token changes the same fields over MCP, and the change is audited under
/// the field's name — a settings change over MCP used to leave no audit record at all.
#[tokio::test]
async fn an_admin_token_changes_privileged_settings_over_mcp() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database, _secrets) = test_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let applied = tool_result(
        &router,
        &session,
        2,
        "update_settings",
        serde_json::json!({ "patch": { "session_max_hours": 48, "session_idle_hours": 6 } }),
    )
    .await;
    assert_eq!(applied["session_max_hours"], 48, "{applied}");
    assert_eq!(applied["session_idle_hours"], 6, "{applied}");

    let records = settings_audit(&database).await;
    let newest = records.first().expect("no settings_changed record");
    assert_eq!(newest["outcome"], "success", "{newest}");
    let fields = newest["details"]["fields"].as_str().expect("fields");
    assert!(fields.contains("session_max_hours"), "{newest}");
    assert!(fields.contains("session_idle_hours"), "{newest}");
}

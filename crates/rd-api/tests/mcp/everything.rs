//! RD-120-32: everything the interface can do, an agent can do — at the price of the route,
//! with the id from a listing tool, and without a secret in any answer.
//!
//! Three things the job's acceptance criteria ask for, each held here rather than in review:
//! every new tool is refused without its route's permission and accepted with exactly it; the
//! listing tools hand out the ids the acting tools take, used together in one flow; and the
//! RD-120-29 canary is searched for in every answer those calls produce.

use axum::Router;
use sha2::{Digest, Sha256};

use super::{API_BEARER, CONFIG_BEARER, QUEUE_BEARER, call, extract_json, handshake, mcp_request};

pub(super) const READ_BEARER: &str = "test-read-bearer-token";
/// Holds credentials and nothing else: `api:secrets` confers no reading, so every tool RD-120-32
/// added refuses it. (`api:metrics` would too, but it does not open the MCP endpoint at all.)
pub(super) const SECRETS_BEARER: &str = "test-secrets-bearer-token";
/// RD-120-55's prices beyond RD-120-32's three: administration, intake, and the metrics scrape.
/// `api:metrics` alone does not open the MCP endpoint, so its token carries reading beside it --
/// which confers nothing towards metrics, so the refusal test still means what it says.
pub(super) const ADMIN_BEARER: &str = "test-admin-bearer-token";
pub(super) const INTAKE_BEARER: &str = "test-intake-bearer-token";
pub(super) const METRICS_BEARER: &str = "test-metrics-bearer-token";
/// Same arrangement as `no_tool_answers_with_a_stored_credential`: nothing else in the
/// installation can produce this string.
pub(super) const CANARY: &str = "rd-120-32-canary-4c7e19a05b3d8f62";
/// A well-formed id no row carries, so a call reaches its handler and is answered there.
pub(super) const NOBODY: &str = "0192f0c4-0000-7000-8000-00000000abcd";

/// The router, with the two extra tokens and the canary behind three credential-bearing rows.
pub(super) async fn installation(directory: &std::path::Path) -> Router {
    installation_parts(directory).await.0
}

/// The same, with the database handle a test seeds rows through.
pub(super) async fn installation_parts(directory: &std::path::Path) -> (Router, rd_db::Database) {
    let (router, database, secrets) = super::test_parts(directory).await;
    for (bearer, scopes) in [
        (READ_BEARER, &[rd_core::API_READ_SCOPE][..]),
        (SECRETS_BEARER, &[rd_core::API_SECRETS_SCOPE][..]),
        (ADMIN_BEARER, &[rd_core::API_ADMIN_SCOPE][..]),
        (INTAKE_BEARER, &[rd_core::API_INTAKE_SCOPE][..]),
        (
            METRICS_BEARER,
            &[rd_core::API_METRICS_SCOPE, rd_core::API_READ_SCOPE][..],
        ),
    ] {
        database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                scopes[0].to_owned(),
                hex::encode(Sha256::digest(bearer.as_bytes())),
                scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            )
            .await
            .expect("token");
    }
    let reference = secrets
        .put_string(CANARY.to_owned())
        .await
        .expect("the vault takes the canary");
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
            secret_ref: Some(reference),
        })
        .await
        .expect("proxy profile");
    (router, database)
}

/// One tool call, answered in full: the JSON-RPC envelope, searched for the canary.
pub(super) async fn envelope(
    router: &Router,
    bearer: &str,
    session: &str,
    name: &str,
    arguments: &serde_json::Value,
) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
    .to_string();
    let (_, content_type, response) =
        call(router, mcp_request(Some(bearer), Some(session), &body)).await;
    assert!(
        !response.contains(CANARY),
        "{name} handed back the stored credential: {response}"
    );
    extract_json(&content_type, &response)
}

/// The parsed payload of a call that must succeed.
pub(super) async fn ok(
    router: &Router,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let answer = envelope(router, API_BEARER, session, name, &arguments).await;
    assert!(answer["error"].is_null(), "{name} was refused: {answer}");
    assert_ne!(answer["result"]["isError"], true, "{name} failed: {answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
}

/// The stable code of a call that must fail inside its handler.
pub(super) async fn refused_with(
    router: &Router,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> String {
    let answer = envelope(router, API_BEARER, session, name, &arguments).await;
    assert_eq!(answer["result"]["isError"], true, "{name}: {answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(text).expect("error JSON");
    parsed["code"].as_str().unwrap_or_default().to_owned()
}

/// Every tool RD-120-32 added, with arguments that reach its handler, and the permission its
/// route costs.
fn new_tools() -> Vec<(&'static str, serde_json::Value, &'static str)> {
    use serde_json::json;
    let (read, queue, config) = ("api:read", "api:queue", "api:config");
    vec![
        ("list_candidates", json!({}), queue),
        ("clear_linkgrabber", json!({}), queue),
        ("delete_candidates", json!({ "ids": [NOBODY] }), queue),
        (
            "move_candidates",
            json!({ "ids": [NOBODY], "new_package_name": "x" }),
            queue,
        ),
        (
            "reorder_candidates",
            json!({ "package_id": NOBODY, "ids": [NOBODY] }),
            queue,
        ),
        (
            "update_candidate",
            json!({ "id": NOBODY, "file_name": "x.bin" }),
            queue,
        ),
        ("enqueue_candidate", json!({ "id": NOBODY }), queue),
        (
            "get_candidate_details",
            json!({ "id": NOBODY, "view": "media" }),
            queue,
        ),
        (
            "set_candidate_plan",
            json!({ "id": NOBODY, "kind": "listing", "body": { "excluded": [] } }),
            queue,
        ),
        (
            "preview_candidate_media",
            json!({ "id": NOBODY, "kind": "output", "body": { "template": "" } }),
            queue,
        ),
        ("resolve_candidate_torrent", json!({ "id": NOBODY }), queue),
        (
            "set_candidate_mirror",
            json!({ "id": NOBODY, "action": "pin" }),
            queue,
        ),
        ("get_mirror_preference", json!({}), queue),
        ("set_mirror_preference", json!({}), queue),
        (
            "update_collector_package",
            json!({ "id": NOBODY, "body": { "priority": "high" } }),
            queue,
        ),
        (
            "update_collector_packages",
            json!({ "definition": { "ids": [NOBODY], "priority": "high" } }),
            queue,
        ),
        ("delete_collector_package", json!({ "id": NOBODY }), queue),
        ("regroup_collector", json!({}), queue),
        (
            "reorder_collector",
            json!({ "entries": [{ "kind": "collector", "id": NOBODY }] }),
            queue,
        ),
        ("list_nzb_imports", json!({}), queue),
        (
            "get_nzb_import",
            json!({ "id": NOBODY, "view": "files" }),
            queue,
        ),
        (
            "update_nzb_import",
            json!({ "id": NOBODY, "body": { "priority": "high" } }),
            queue,
        ),
        ("enqueue_nzb_import", json!({ "id": NOBODY }), queue),
        ("delete_nzb_import", json!({ "id": NOBODY }), queue),
        ("reorder_packages", json!({ "ids": [NOBODY] }), queue),
        (
            "reorder_downloads",
            json!({ "package_id": NOBODY, "ids": [NOBODY] }),
            queue,
        ),
        (
            "rename_download",
            json!({ "id": NOBODY, "name": "x.bin" }),
            queue,
        ),
        (
            "update_package",
            json!({ "id": NOBODY, "body": { "priority": "high" } }),
            queue,
        ),
        (
            "update_packages",
            json!({ "definition": { "ids": [NOBODY], "priority": "high" } }),
            queue,
        ),
        (
            "rename_package_folder",
            json!({ "id": NOBODY, "name": "x" }),
            queue,
        ),
        (
            "clear_finished_packages",
            json!({ "scope": "completed" }),
            queue,
        ),
        ("extract_downloads", json!({ "ids": [NOBODY] }), queue),
        ("extract_packages", json!({ "ids": [NOBODY] }), queue),
        ("get_package_postprocess", json!({ "id": NOBODY }), read),
        (
            "get_torrent_details",
            json!({ "id": NOBODY, "view": "summary" }),
            read,
        ),
        ("set_torrent_file_plan", json!({ "id": NOBODY }), queue),
        (
            "update_torrent_trackers",
            json!({ "id": NOBODY, "action": "reannounce" }),
            queue,
        ),
        (
            "set_torrent_seeding",
            json!({ "id": NOBODY, "clear": true }),
            queue,
        ),
        ("stop_seeding", json!({ "id": NOBODY }), queue),
        (
            "set_category_seeding",
            json!({ "id": NOBODY, "clear": true }),
            config,
        ),
        (
            "get_torrent_engine",
            json!({ "view": "capabilities" }),
            read,
        ),
        ("list_network_interfaces", json!({}), config),
        (
            "list_postprocess_options",
            json!({ "kind": "upload_destinations" }),
            queue,
        ),
        ("list_postprocess_queue", json!({}), read),
        (
            "update_category_postprocess",
            json!({ "id": NOBODY, "body": {} }),
            config,
        ),
        ("list_managed_tools", json!({ "view": "media" }), read),
        (
            "manage_tool",
            json!({ "name": "nothing-like-this", "action": "rollback" }),
            config,
        ),
        ("refresh_tool_manifest", json!({}), config),
        ("get_storage_capacity", json!({}), read),
        (
            "resume_storage_target",
            json!({ "target": "fallback" }),
            queue,
        ),
        ("create_site_rule", json!({ "rule": {} }), config),
        (
            "update_site_rule",
            json!({ "id": "nothing-like-this", "rule": {} }),
            config,
        ),
        (
            "delete_site_rule",
            json!({ "id": "nothing-like-this" }),
            config,
        ),
        (
            "test_site_rule",
            json!({ "rule": {}, "address": "http://127.0.0.1:9/" }),
            config,
        ),
    ]
}

/// Each new tool is refused by a token one step short of its route's permission, names that
/// permission, and is accepted by a token holding exactly it.
///
/// The near miss is chosen per price so the refusal is not trivially won: `api:queue` implies
/// reading, so it is the token that must not reach a configuration tool; `api:config` implies
/// reading too, so it must not reach a queue tool; `api:secrets` confers no reading, so it must
/// not reach a reading one.
#[tokio::test]
async fn every_new_tool_costs_what_its_route_costs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let tools = new_tools();

    let mut names: Vec<&str> = tools.iter().map(|(name, _, _)| *name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 54, "RD-120-32 added 54 tools");

    let full = handshake(&router, API_BEARER).await;
    let listed = envelope(
        &router,
        API_BEARER,
        &full,
        "list_candidates",
        &serde_json::json!({}),
    )
    .await;
    assert!(listed["error"].is_null(), "{listed}");

    let sessions = [
        (READ_BEARER, handshake(&router, READ_BEARER).await),
        (QUEUE_BEARER, handshake(&router, QUEUE_BEARER).await),
        (CONFIG_BEARER, handshake(&router, CONFIG_BEARER).await),
        (SECRETS_BEARER, handshake(&router, SECRETS_BEARER).await),
    ];
    let session = |bearer: &str| -> &str {
        &sessions
            .iter()
            .find(|(held, _)| *held == bearer)
            .expect("a session per token")
            .1
    };

    for (name, arguments, scope) in &tools {
        let (short, exact) = match *scope {
            "api:read" => (SECRETS_BEARER, READ_BEARER),
            "api:queue" => (CONFIG_BEARER, QUEUE_BEARER),
            "api:config" => (QUEUE_BEARER, CONFIG_BEARER),
            other => panic!("no near miss for {other}"),
        };
        let answer = envelope(&router, short, session(short), name, arguments).await;
        assert_eq!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{name} was not refused without {scope}: {answer}"
        );
        assert_eq!(
            answer["error"]["data"]["scope"], *scope,
            "{name} named the wrong permission: {answer}"
        );

        // The manifest refresh goes out to the network; its price is proven by the refusal.
        if *name == "refresh_tool_manifest" {
            continue;
        }
        let answer = envelope(&router, exact, session(exact), name, arguments).await;
        assert_ne!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{name} was refused with exactly {scope}: {answer}"
        );
        assert!(
            answer["result"].is_object(),
            "{name} did not reach its handler: {answer}"
        );
    }
}

#[path = "everything_flow.rs"]
mod flow;

#[path = "everything_remaining.rs"]
mod remaining;

#[path = "everything_remaining_flow.rs"]
mod remaining_flow;

#[path = "everything_indexer_key.rs"]
mod indexer_key;

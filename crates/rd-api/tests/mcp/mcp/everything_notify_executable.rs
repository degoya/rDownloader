//! RD-1101-11 (audit 2026-10-05, S1) over MCP: the target tools cost `api:config`, and the
//! program an apprise destination runs costs `api:admin` on top, as it does over REST — the
//! tools reach the same `save_target` with the caller's own grant.

use serde_json::json;

use super::{API_BEARER, CONFIG_BEARER, envelope, handshake, installation, ok};

/// The stable code and the scope of a tool call that must fail inside its handler.
async fn refusal(
    router: &axum::Router,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let answer = envelope(router, CONFIG_BEARER, session, name, &arguments).await;
    assert_eq!(answer["result"]["isError"], true, "{name}: {answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text).expect("error JSON")
}

/// The parsed payload of a call the configuration token may make.
async fn as_config(
    router: &axum::Router,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let answer = envelope(router, CONFIG_BEARER, session, name, &arguments).await;
    assert!(answer["error"].is_null(), "{name} was refused: {answer}");
    assert_ne!(answer["result"]["isError"], true, "{name} failed: {answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text).expect("tool JSON")
}

#[tokio::test]
async fn a_configuration_token_cannot_name_a_destination_s_program_over_mcp() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let config_session = handshake(&router, CONFIG_BEARER).await;
    let named = json!({
        "name": "Phone",
        "kind": "apprise",
        "endpoint": "tgram",
        "config": { "executable": "/bin/sh" }
    });

    let refused = refusal(
        &router,
        &config_session,
        "create_notification_target",
        named.clone(),
    )
    .await;
    assert_eq!(refused["code"], "auth.scope_insufficient", "{refused}");
    assert_eq!(refused["params"]["scope"], "api:admin", "{refused}");

    // Without a path the same token creates the destination, and cannot add one afterwards.
    let plain = as_config(
        &router,
        &config_session,
        "create_notification_target",
        json!({ "name": "Phone", "kind": "apprise", "endpoint": "tgram" }),
    )
    .await;
    let id = plain["id"].as_str().expect("id").to_owned();
    let refused = refusal(
        &router,
        &config_session,
        "update_notification_target",
        json!({ "id": id, "config": { "executable": "/bin/sh" } }),
    )
    .await;
    assert_eq!(refused["code"], "auth.scope_insufficient", "{refused}");

    // An administrator names it; a configuration token may then change everything else.
    let admin_session = handshake(&router, API_BEARER).await;
    // Another name: target names are unique, and "Phone" exists since the plain create above.
    let mut named = named;
    named["name"] = json!("Phone with a program");
    let created = ok(&router, &admin_session, "create_notification_target", named).await;
    assert_eq!(created["config"]["executable"], "/bin/sh", "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    let renamed = as_config(
        &router,
        &config_session,
        "update_notification_target",
        json!({ "id": id, "name": "Renamed" }),
    )
    .await;
    assert_eq!(renamed["name"], "Renamed", "{renamed}");
    assert_eq!(renamed["config"]["executable"], "/bin/sh", "{renamed}");
}

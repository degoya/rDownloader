//! RD-1200-04 (`docs/security/mcp.md` findings 6 and 8): an action taken through a tool is
//! audited as `mcp`, the same action over REST as `rest`, and a token's call limit holds on the
//! MCP endpoint as it does on the REST routes.

use axum::http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{API_BEARER, call, envelope, handshake, installation_parts, mcp_request, ok};
use crate::common;

/// The `settings_changed` records, newest first.
async fn settings_changes(database: &rd_db::Database) -> Vec<rd_db::AuditRecord> {
    database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::SettingsChanged),
            limit: 20,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
}

#[tokio::test]
async fn a_tool_call_is_audited_as_mcp_and_the_same_change_over_rest_as_rest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let applied = ok(
        &router,
        &session,
        "update_settings",
        json!({ "patch": { "max_active_files": 3 } }),
    )
    .await;
    assert_eq!(applied["max_active_files"], 3, "{applied}");
    let through_mcp = settings_changes(&database).await;
    let newest = through_mcp.first().expect("a settings_changed record");
    assert_eq!(newest.actor_kind, rd_core::AuditActorKind::Token);
    assert_eq!(newest.via, rd_core::AuditChannel::Mcp, "{newest:?}");

    let (status, mut settings) =
        common::get_with_bearer(&router, "/api/v1/settings", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["max_active_files"] = json!(4);
    let (status, saved) =
        common::put_with_bearer(&router, "/api/v1/settings", API_BEARER, settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let both = settings_changes(&database).await;
    let newest = both.first().expect("the REST record");
    assert_ne!(
        newest.id, through_mcp[0].id,
        "the REST change wrote its own record"
    );
    assert_eq!(newest.actor_id, through_mcp[0].actor_id, "the same token");
    assert_eq!(newest.via, rd_core::AuditChannel::Rest, "{newest:?}");

    // The audit tool shows the channel and filters by it.
    let listed = ok(
        &router,
        &session,
        "list_audit_records",
        json!({ "action": "settings_changed", "via": "mcp" }),
    )
    .await;
    let records = listed["records"].as_array().expect("records");
    assert!(!records.is_empty(), "{listed}");
    assert!(
        records.iter().all(|record| record["via"] == "mcp"),
        "{listed}"
    );
}

#[tokio::test]
async fn a_token_over_its_limit_is_refused_on_the_mcp_endpoint() {
    const LIMITED: &str = "test-limited-mcp-bearer-token";
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    database
        .create_limited_capture_token(
            rd_core::CaptureTokenId::new(),
            "limited agent".to_owned(),
            hex::encode(Sha256::digest(LIMITED.as_bytes())),
            vec![rd_core::API_READ_SCOPE.to_owned()],
            None,
            Some(4),
        )
        .await
        .expect("token");

    // The handshake is two messages; two tool calls fill the minute.
    let session = handshake(&router, LIMITED).await;
    for _ in 0..2 {
        let answer = envelope(&router, LIMITED, &session, "get_status_summary", &json!({})).await;
        assert!(answer["error"].is_null(), "{answer}");
        assert_ne!(answer["result"]["isError"], true, "{answer}");
    }
    let body = json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": { "name": "get_status_summary", "arguments": {} }
    })
    .to_string();
    let (status, _, refused) =
        call(&router, mcp_request(Some(LIMITED), Some(&session), &body)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    let refused: serde_json::Value = serde_json::from_str(&refused).expect("json");
    assert_eq!(refused["code"], "api.token_rate_limited", "{refused}");
    assert_eq!(refused["params"]["limit"], "4", "{refused}");

    // Another token's calls are its own.
    let other = handshake(&router, API_BEARER).await;
    ok(&router, &other, "get_status_summary", json!({})).await;
}

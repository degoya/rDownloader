//! RD-1190-21 over MCP: what a tool may make the service run, the question a clearing tool asks
//! first, the notice on third parties' text, the webhook address a tool shows, and a container
//! larger than rmcp's own body limit.

use axum::{Router, http::StatusCode};
use serde_json::json;

use super::{
    API_BEARER, CONFIG_BEARER, call, envelope, extract_json, handshake, installation, mcp_request,
    ok, refused_with,
};
use crate::common;

/// `mcp_scripts_allowed` switched the way the person does it: the settings document over REST.
async fn allow_scripts(router: &Router, allowed: bool) {
    let (status, mut settings) =
        common::get_with_bearer(router, "/api/v1/settings", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["mcp_scripts_allowed"] = json!(allowed);
    let (status, saved) =
        common::put_with_bearer(router, "/api/v1/settings", API_BEARER, settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["mcp_scripts_allowed"], allowed, "{saved}");
}

#[tokio::test]
async fn a_tool_names_a_script_only_when_the_person_allowed_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;
    let root = ok(
        &router,
        &session,
        "create_storage_root",
        json!({ "name": "Downloads", "path": directory.path().join("roots/main").to_string_lossy() }),
    )
    .await;
    let category = json!({
        "name": "Films", "color": "#112233", "storage_root_id": root["id"],
        "relative_path": "films", "script": "tidy.sh",
    });
    let automation = json!({ "definition": {
        "name": "Tidy up", "trigger": "download_completed", "condition": { "type": "always" },
        "actions": [{ "kind": "script", "name": "tidy.sh" }],
    }});

    // Off by default: every place a tool could name a script refuses it, whatever the token holds.
    for (name, arguments) in [
        ("create_category", category.clone()),
        ("create_automation", automation.clone()),
        (
            "update_settings",
            json!({ "patch": { "completion_script": "tidy.sh" } }),
        ),
    ] {
        let code = refused_with(&router, &session, name, arguments).await;
        assert_eq!(code, "mcp.script_not_allowed", "{name}");
    }
    // No tool switches it on, and over REST it costs administration.
    let code = refused_with(
        &router,
        &session,
        "update_settings",
        json!({ "patch": { "mcp_scripts_allowed": true } }),
    )
    .await;
    assert_eq!(code, "mcp.setting_outside_mcp");
    let (_, mut settings) =
        common::get_with_bearer(&router, "/api/v1/settings", CONFIG_BEARER).await;
    settings["mcp_scripts_allowed"] = json!(true);
    let (status, refused) =
        common::put_with_bearer(&router, "/api/v1/settings", CONFIG_BEARER, settings).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(
        refused["params"]["setting"], "mcp_scripts_allowed",
        "{refused}"
    );

    // Allowed by the person, the same calls go through.
    allow_scripts(&router, true).await;
    let created = ok(&router, &session, "create_category", category).await;
    assert_eq!(created["script"], "tidy.sh", "{created}");
    let id = created["id"].as_str().expect("category id").to_owned();
    ok(&router, &session, "create_automation", automation).await;

    // Switched off again: the script the category carries may be passed back, a new one not,
    // and clearing one starts nothing.
    allow_scripts(&router, false).await;
    let kept = ok(
        &router,
        &session,
        "update_category",
        json!({ "id": id, "script": "tidy.sh", "color": "#445566" }),
    )
    .await;
    assert_eq!(kept["color"], "#445566", "{kept}");
    let code = refused_with(
        &router,
        &session,
        "update_category",
        json!({ "id": id, "script": "other.sh" }),
    )
    .await;
    assert_eq!(code, "mcp.script_not_allowed");
    let cleared = ok(
        &router,
        &session,
        "update_category",
        json!({ "id": id, "clear": ["script"] }),
    )
    .await;
    assert!(cleared["script"].is_null(), "{cleared}");
}

#[tokio::test]
async fn a_clearing_tool_acts_only_on_the_code_its_own_question_handed_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    for tool in [
        "clear_log_records",
        "clear_audit_records",
        "clear_transfer_stats",
        "clear_notification_deliveries",
        "discard_pending_notification_deliveries",
        "clear_storage_operations",
        "clear_content_index",
        "clear_download_history",
        "clear_remote_jobs",
        "clean_up_data_directory",
    ] {
        // `confirmed` alone is the model's own word: the tool asks instead of acting.
        let asked = ok(&router, &session, tool, json!({ "confirmed": true })).await;
        assert_eq!(asked["confirmation_required"], true, "{tool}: {asked}");
        assert_eq!(asked["tool"], tool, "{asked}");
        let code = asked["confirmation"].as_str().expect("a code").to_owned();
        let invented = json!({ "confirmed": true, "confirmation": "00000000" });
        let again = ok(&router, &session, tool, invented).await;
        assert_eq!(again["confirmation_required"], true, "{tool}: {again}");

        let answered = json!({ "confirmed": true, "confirmation": code });
        let done = ok(&router, &session, tool, answered.clone()).await;
        assert_ne!(done["confirmation_required"], true, "{tool}: {done}");
        let spent = ok(&router, &session, tool, answered).await;
        assert_eq!(
            spent["confirmation_required"], true,
            "{tool}: a code answers once"
        );
    }

    // A code belongs to the session whose tool asked.
    let other = handshake(&router, API_BEARER).await;
    let asked = ok(
        &router,
        &session,
        "clear_log_records",
        json!({ "confirmed": true }),
    )
    .await;
    let foreign = ok(
        &router,
        &other,
        "clear_log_records",
        json!({ "confirmed": true, "confirmation": asked["confirmation"] }),
    )
    .await;
    assert_eq!(foreign["confirmation_required"], true, "{foreign}");
}

#[tokio::test]
async fn an_answer_quoting_third_parties_ends_with_the_untrusted_notice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let marked = envelope(&router, API_BEARER, &session, "list_downloads", &json!({})).await;
    let content = marked["result"]["content"].as_array().expect("content");
    assert_eq!(content.len(), 2, "{marked}");
    let data = content[0]["text"].as_str().unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(data).expect("the data stays the first block");
    let notice = content[1]["text"].as_str().unwrap_or_default();
    assert!(notice.starts_with("[untrusted content]"), "{notice}");

    let plain = envelope(&router, API_BEARER, &session, "get_settings", &json!({})).await;
    assert_eq!(
        plain["result"]["content"].as_array().map(Vec::len),
        Some(1),
        "{plain}"
    );

    let body = json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" }).to_string();
    let (_, content_type, response) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &body),
    )
    .await;
    let listed = extract_json(&content_type, &response);
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let described = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .and_then(|tool| tool["description"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    assert!(described("search_indexers").contains("[untrusted content]"));
    assert!(!described("get_settings").contains("[untrusted content]"));
}

#[tokio::test]
async fn a_webhook_destination_is_answered_without_the_key_in_its_path() {
    const KEY: &str = "rd-1190-21-hook-5e8a7c";
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;
    let hook = format!("https://hooks.slack.com/services/T0/B0/{KEY}");

    let created = ok(
        &router,
        &session,
        "create_notification_target",
        json!({ "name": "Team chat", "kind": "webhook", "endpoint": hook }),
    )
    .await;
    assert_eq!(
        created["endpoint"], "https://hooks.slack.com/[redacted]",
        "{created}"
    );
    let id = created["id"].as_str().expect("target id").to_owned();
    let listed = envelope(
        &router,
        API_BEARER,
        &session,
        "list_notification_targets",
        &json!({}),
    )
    .await;
    assert!(!listed.to_string().contains(KEY), "{listed}");

    // Passed back as the tool showed it, the address stands for the stored one.
    let renamed = ok(
        &router,
        &session,
        "update_notification_target",
        json!({ "id": id, "name": "Team", "endpoint": "https://hooks.slack.com/[redacted]" }),
    )
    .await;
    assert_eq!(renamed["name"], "Team", "{renamed}");
    let (status, stored) =
        common::get_with_bearer(&router, "/api/v1/notifications/targets", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored[0]["endpoint"], hook, "the person's address is kept");
}

/// rmcp refused any `POST` above 4 MiB on its own, while `import_container` promises 48 MiB.
#[tokio::test]
async fn a_file_above_four_mib_reaches_its_tool() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;
    let body = json!({
        "jsonrpc": "2.0", "id": 11, "method": "tools/call",
        "params": { "name": "import_torrent", "arguments": {
            "content": "A".repeat(6 * 1024 * 1024), "file_name": "large.torrent",
        }},
    })
    .to_string();
    let (status, content_type, response) = call(
        &router,
        mcp_request(Some(API_BEARER), Some(&session), &body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the transport refused the body");
    let answer = extract_json(&content_type, &response);
    assert!(answer["error"].is_null(), "{answer}");
    // Six MiB of zero bytes is no torrent: refused by the tool, not by the transport.
    assert_eq!(answer["result"]["isError"], true, "{answer}");
}

/// RD-1190-22: every `POST` asks for the token again, so revoking it ends what an open session
/// may do at its next call.
#[tokio::test]
async fn a_revoked_token_ends_its_mcp_session() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = super::installation_parts(directory.path()).await;
    let session = handshake(&router, common::READ_BEARER).await;
    let list = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }).to_string();
    let (status, _, answer) = call(
        &router,
        mcp_request(Some(common::READ_BEARER), Some(&session), &list),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    for token in database
        .list_capture_tokens(&[rd_core::API_READ_SCOPE])
        .await
        .expect("tokens")
    {
        database
            .revoke_capture_token(token.id)
            .await
            .expect("revoke");
    }
    let (status, _, answer) = call(
        &router,
        mcp_request(Some(common::READ_BEARER), Some(&session), &list),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
}

/// RD-1200-01, owner 2026-10-08: `clear_remote_jobs` clears this installation's list and never
/// deletes at a provider, whatever the call asks. The job names a plugin nothing installs, so a
/// discard would fail and keep its row; the row going with nothing failed is the proof that no
/// discard was attempted.
#[tokio::test]
async fn clearing_remote_jobs_over_mcp_never_reaches_a_provider() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = super::installation_parts(directory.path()).await;
    let account = database
        .create_account(rd_db::NewAccount {
            provider: "torbox".to_owned(),
            label: "torbox mcp clear".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let id = rd_core::RemoteJobId::new();
    database
        .claim_remote_job(rd_db::ClaimRemoteJob {
            id,
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-0000000012c1".to_owned(),
            content_key: format!("mcp-clear-{id}"),
            source_kind: rd_core::RemoteJobSourceKind::Magnet,
            source: b"magnet:?xt=urn:btih:mcp-clear".to_vec(),
            source_name: None,
            package_id: None,
        })
        .await
        .expect("claim");
    database
        .advance_remote_job(
            id,
            rd_db::AdvanceRemoteJob {
                state: Some(rd_core::RemoteJobState::Ready),
                remote_id: Some("TB-MCP".to_owned()),
                next_poll_at: Some(Some(chrono::Utc::now() + chrono::Duration::days(1))),
                ..rd_db::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("advance");
    let session = handshake(&router, API_BEARER).await;

    let asking = json!({ "confirmed": true, "at_provider": true });
    let asked = ok(&router, &session, "clear_remote_jobs", asking).await;
    assert!(
        asked["question"]
            .as_str()
            .is_some_and(|text| text.contains("nothing is deleted at the provider")),
        "{asked}"
    );
    let code = asked["confirmation"].as_str().expect("a code").to_owned();
    let answered = json!({ "confirmed": true, "at_provider": true, "confirmation": code });
    let done = ok(&router, &session, "clear_remote_jobs", answered).await;
    assert_eq!(done["removed"], 1, "{done}");
    assert_eq!(done["failed"], 0, "{done}");
    assert!(database.remote_job(id).await.expect("read").is_none());

    let records = database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::RemoteJobsCleared),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert_eq!(
        records
            .first()
            .and_then(|record| record.details.get("at_provider"))
            .map(String::as_str),
        Some("false")
    );
}

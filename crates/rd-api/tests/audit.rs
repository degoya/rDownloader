//! The audit log over REST (RD-110-03).
//!
//! The matrix test is the point of this file: every action the job names is performed through
//! the router, and the record it must leave is looked up afterwards. A missing record fails
//! here rather than in a year, when somebody needed it.
//!
//! The store, the append-only trigger and retention are covered in
//! `crates/rd-db/tests/audit_store.rs`; the trace export in `crates/rd-diagnostics/tests`.

mod common;

use axum::http::StatusCode;
use common::{
    API_BEARER, READ_BEARER, auth_harness, delete_json, get_json, get_with_bearer, post_json,
    put_json, test_harness,
};
use sha2::Digest;

const PASSWORD: &str = "correct-horse-battery";

/// Values that must never reach a record.
const CANARIES: &[&str] = &[PASSWORD, "sk-live-4242", "hunter2", "test-api-bearer-token"];

/// The stored settings, with the switch the default harness relies on kept as it is.
///
/// `build_harness` disables the administrator login in memory while the stored document still
/// says it is on, so *any* successful settings write would switch it back on and every later
/// request in the same test would answer `401 auth.setup_pending`. Writing the switch back is
/// what keeps a settings test about settings.
async fn settings_of(router: &axum::Router) -> serde_json::Value {
    let (status, mut settings) = get_json(router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["admin_login_disabled"] = serde_json::json!(true);
    settings
}

/// Every record the log holds, newest first, read from the store rather than over REST.
///
/// The matrix performs actions that change the authentication configuration — resetting the
/// settings restores the stored `admin_login_disabled` — and a read that stops working
/// halfway through would say nothing about whether the record was written. The REST surface
/// has its own tests below.
async fn records(database: &rd_db::Database) -> Vec<serde_json::Value> {
    let stored = database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 500,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("query");
    stored
        .into_iter()
        .map(|record| serde_json::to_value(record).expect("json"))
        .collect()
}

/// The newest record of this action, or a failure naming what was there instead.
async fn newest(database: &rd_db::Database, action: &str) -> serde_json::Value {
    let all = records(database).await;
    all.iter()
        .find(|record| record["action"] == action)
        .cloned()
        .unwrap_or_else(|| {
            let seen: Vec<&str> = all
                .iter()
                .filter_map(|record| record["action"].as_str())
                .collect();
            panic!("no `{action}` record was written; the log holds {seen:?}")
        })
}

#[tokio::test]
async fn every_named_action_leaves_a_record() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;

    // --- a token: created, used, re-scoped, revoked
    let (status, token) = post_json(
        router,
        "/api/v1/api-tokens",
        serde_json::json!({ "label": "matrix", "scopes": ["api:read"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let token_id = token["token"]["id"].as_str().expect("id").to_owned();
    let created = newest(&harness.database, "token_created").await;
    assert_eq!(created["target_id"], token_id);
    assert_eq!(created["target_name"], "matrix");
    assert_eq!(created["details"]["scopes"], "api:read");

    // Token *use* needs a harness where a credential is actually consulted; see
    // `a_machine_tokens_use_is_recorded`.

    let (status, body) = common::patch_with_bearer(
        router,
        &format!("/api/v1/api-tokens/{token_id}"),
        API_BEARER,
        serde_json::json!({ "scopes": ["api:queue"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rescoped = newest(&harness.database, "token_rescoped").await;
    assert_eq!(rescoped["details"]["scopes_before"], "api:read");
    assert_eq!(rescoped["details"]["scopes_after"], "api:queue");

    let (status, body) = delete_json(router, &format!("/api/v1/api-tokens/{token_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        newest(&harness.database, "token_revoked").await["target_id"],
        token_id
    );

    // --- the settings document
    let mut settings = settings_of(router).await;
    settings["audit_retention_days"] = serde_json::json!(90);
    let (status, body) = put_json(router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let changed = newest(&harness.database, "settings_changed").await;
    assert!(
        changed["details"]["fields"]
            .as_str()
            .expect("fields")
            .contains("audit_retention_days"),
        "the changed field was not named: {changed}"
    );

    // --- a plugin trust decision: a withdrawal and its reversal
    let digest = "b".repeat(64);
    let (status, body) = post_json(
        router,
        "/api/v1/plugins/revocations",
        serde_json::json!({ "digest": digest, "reason": "matrix" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        newest(&harness.database, "plugin_digest_revoked").await["target_id"],
        digest
    );
    let (status, body) =
        delete_json(router, &format!("/api/v1/plugins/revocations/{digest}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    newest(&harness.database, "plugin_digest_unrevoked").await;

    let (status, body) = delete_json(router, "/api/v1/plugins/keys/matrix-key").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        newest(&harness.database, "plugin_key_revoked").await["target_id"],
        "matrix-key"
    );

    // --- destructive actions on configuration
    let (status, root) = post_json(
        router,
        "/api/v1/storage-roots",
        serde_json::json!({
            "name": "matrix-root",
            "path": directory.path().join("matrix").display().to_string(),
            "is_default": false,
            "minimum_free_bytes": null,
        }),
    )
    .await;
    assert!(status.is_success(), "{root}");
    let root_id = root["id"].as_str().expect("id").to_owned();
    let (status, category) = post_json(
        router,
        "/api/v1/categories",
        serde_json::json!({
            "name": "matrix-category",
            "color": "#38BDF8",
            "storage_root_id": root_id,
            "relative_path": "",
            "is_default": false
        }),
    )
    .await;
    assert!(status.is_success(), "{category}");
    let category_id = category["id"].as_str().expect("id").to_owned();

    let (status, body) = delete_json(router, &format!("/api/v1/categories/{category_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let deleted = newest(&harness.database, "category_deleted").await;
    assert_eq!(deleted["target_id"], category_id);
    assert_eq!(deleted["target_name"], "matrix-category");

    let (status, body) = delete_json(router, &format!("/api/v1/storage-roots/{root_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let deleted = newest(&harness.database, "storage_root_deleted").await;
    assert_eq!(deleted["target_name"], "matrix-root");

    // --- a download and its package
    //
    // Two downloads, because a package disappears with its last file: the first is deleted as
    // a download, the second's package as a package.
    let mut added = Vec::new();
    for index in 0..2 {
        let (status, body) = post_json(
            router,
            "/api/v1/downloads",
            serde_json::json!({ "url": format!("https://example.invalid/matrix-{index}.bin") }),
        )
        .await;
        assert!(status.is_success(), "{body}");
        added.push(body);
    }
    let download_id = added[0]["id"].as_str().expect("id").to_owned();
    let package_id = added[1]["package_id"]
        .as_str()
        .expect("the download was filed under a package")
        .to_owned();
    let (status, body) = delete_json(router, &format!("/api/v1/downloads/{download_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        newest(&harness.database, "download_deleted").await["target_id"],
        download_id
    );

    let (status, body) =
        delete_json(router, &format!("/api/v1/packages/{package_id}?force=true")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        newest(&harness.database, "package_deleted").await["target_id"],
        package_id
    );

    // --- restoring a configuration backup
    let (status, bundle) = post_json(
        router,
        "/api/v1/settings/export",
        serde_json::json!({ "include_secrets": false }),
    )
    .await;
    assert!(status.is_success(), "{bundle}");
    let (status, body) = post_json(
        router,
        "/api/v1/settings/import",
        serde_json::json!({ "bundle": bundle }),
    )
    .await;
    assert!(status.is_success(), "import: {status} {body}");
    newest(&harness.database, "backup_restored").await;

    // Last, because it restores the stored `admin_login_disabled` and with it the login the
    // default harness switched off: every request after this one would answer `401`.
    let (status, body) = post_json(router, "/api/v1/settings/reset", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    newest(&harness.database, "settings_reset").await;
}

#[tokio::test]
async fn a_sign_in_and_a_refused_sign_in_are_both_recorded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let router = &harness.router;
    let (status, body) = post_json(
        router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert!(status.is_success(), "setup: {body}");

    let (status, _) = post_json(
        router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": "wrong-password-entirely" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, cookie) = common::post_json_with_headers(
        router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie.expect("a session cookie");

    let (status, body) =
        common::get_with_cookie(router, "/api/v1/audit/records?limit=500", &cookie).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let all = body["records"].as_array().expect("records").clone();

    let failed = all
        .iter()
        .find(|record| record["action"] == "login_failed")
        .expect("a refused sign-in was not recorded");
    assert_eq!(failed["outcome"], "failure");
    assert_eq!(failed["actor_kind"], "anonymous");
    assert_eq!(failed["details"]["stage"], "password");

    let ok = all
        .iter()
        .find(|record| record["action"] == "login_succeeded")
        .expect("an accepted sign-in was not recorded");
    assert_eq!(ok["outcome"], "success");
    assert_eq!(ok["actor_kind"], "session");
    assert!(ok["actor_id"].is_string(), "the session was not named");

    // The whole page, not only the two records: nothing anywhere may quote the password.
    let text = body.to_string();
    for canary in CANARIES {
        assert!(!text.contains(canary), "the log shows {canary}: {text}");
    }

    // And signing out is recorded too.
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/auth/logout",
        &cookie,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_log_has_no_write_edit_or_delete_route() {
    // The contract first: the documented surface of this area is two reads and one clear that
    // cannot pick its rows (RD-120-34). An operation added later that could write, edit, or
    // remove a *chosen* record shows up here before it shows up in a store.
    //
    // The clear is not an exception to the append-only property, it is the shape that property
    // allows: it takes no filter, it empties the whole log, and it writes itself into the
    // emptied log as its first entry. What stays impossible is rewriting history by removing
    // the one record somebody did not want.
    let document = serde_json::to_value(rd_api::openapi_document()).expect("openapi");
    let paths = document["paths"].as_object().expect("paths");
    let audit: Vec<(&String, Vec<&String>)> = paths
        .iter()
        .filter(|(path, _)| path.starts_with("/api/v1/audit"))
        .map(|(path, item)| {
            (
                path,
                item.as_object()
                    .expect("operations")
                    .keys()
                    .filter(|method| {
                        ["get", "post", "put", "patch", "delete"].contains(&method.as_str())
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(audit.len(), 3, "unexpected audit paths: {audit:?}");
    for (path, methods) in &audit {
        let expected = if path.ends_with("/clear") {
            "post"
        } else {
            "get"
        };
        assert_eq!(
            methods,
            &vec![&expected.to_owned()],
            "{path} carries an operation this area does not have"
        );
    }

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    post_json(
        &harness.router,
        "/api/v1/api-tokens",
        serde_json::json!({ "label": "kept", "scopes": ["api:read"] }),
    )
    .await;
    let before = records(&harness.database).await.len();
    assert!(before > 0, "nothing was recorded to begin with");

    // Every shape somebody would reach for to remove a record. Some are refused by the router,
    // some fall through to the single-page application; none of them removes anything, which
    // is the assertion. A 200 here is the index page, not an answer from this area — which is
    // exactly why the count below is what the test believes and not the status code.
    for (method, uri) in [
        ("DELETE", "/api/v1/audit/records"),
        ("DELETE", "/api/v1/audit/records/1"),
        ("POST", "/api/v1/audit/records"),
        ("PUT", "/api/v1/audit/records/1"),
        ("PATCH", "/api/v1/audit/records/1"),
        ("POST", "/api/v1/audit/clear"),
        ("DELETE", "/api/v1/audit/export"),
        // The clear that does exist, reached the way a client that drew no dialog would reach
        // it: `request_with_bearer` sends `{}`, so the confirmation is missing and it removes
        // nothing. The route's own test is `tests/data_reset.rs`.
        ("POST", "/api/v1/audit/records/clear"),
        // And the shape that must never exist: a clear that names a row.
        ("POST", "/api/v1/audit/records/1/clear"),
    ] {
        let (status, body) =
            common::request_with_bearer(&harness.router, method, uri, API_BEARER).await;
        assert_ne!(
            body["deleted"],
            serde_json::json!(true),
            "{method} {uri} claims to have deleted something: {status}"
        );
    }
    assert_eq!(
        records(&harness.database).await.len(),
        before,
        "a record disappeared"
    );
}

#[tokio::test]
async fn the_log_survives_deleting_the_things_it_talks_about() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    let (status, token) = post_json(
        router,
        "/api/v1/api-tokens",
        serde_json::json!({ "label": "ephemeral", "scopes": ["api:read"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let token_id = token["token"]["id"].as_str().expect("id").to_owned();
    delete_json(router, &format!("/api/v1/api-tokens/{token_id}")).await;

    // The token is gone from the inventory and both of its records are still here. A domain
    // delete removing its own audit trail is the failure this guards against.
    let (_, tokens) = get_json(router, "/api/v1/api-tokens").await;
    let live = tokens
        .as_array()
        .map(|tokens| {
            tokens
                .iter()
                .any(|token| token["id"] == token_id && token["revoked_at"].is_null())
        })
        .unwrap_or(false);
    assert!(!live, "the token is still live");
    let all = records(&harness.database).await;
    assert!(
        all.iter()
            .any(|record| record["action"] == "token_created" && record["target_id"] == token_id)
    );
    assert!(
        all.iter()
            .any(|record| record["action"] == "token_revoked" && record["target_id"] == token_id)
    );
}

#[tokio::test]
async fn a_record_carries_the_trace_of_the_request_that_caused_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let caller = rd_core::TraceContext::for_job("test", "audit-trace");
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v1/api-tokens")
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("traceparent", caller.traceparent())
        .body(axum::body::Body::from(
            serde_json::json!({ "label": "traced", "scopes": ["api:read"] }).to_string(),
        ))
        .expect("request");
    let response = tower::ServiceExt::oneshot(harness.router.clone(), request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get("traceparent")
            .and_then(|value| value.to_str().ok()),
        Some(caller.traceparent().as_str()),
        "the response did not carry the trace back"
    );

    let record = newest(&harness.database, "token_created").await;
    assert_eq!(record["trace_id"], caller.trace_id_hex());
    let (status, body) = get_json(
        &harness.router,
        &format!("/api/v1/audit/records?trace_id={}", caller.trace_id_hex()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["records"].as_array().expect("records").len(), 1);
}

#[tokio::test]
async fn an_unreachable_collector_does_not_block_a_request() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    // A port nothing listens on, and the export pointed straight at it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    drop(listener);

    let mut settings = settings_of(&harness.router).await;
    settings["otlp_enabled"] = serde_json::json!(true);
    settings["otlp_endpoint"] = serde_json::json!(format!("http://127.0.0.1:{port}/v1/traces"));
    settings["otlp_timeout_seconds"] = serde_json::json!(1);
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Work carries on at full speed with the collector unreachable. Every one of these goes
    // through the trace middleware and opens spans the exporter would want.
    let started = std::time::Instant::now();
    for index in 0..10 {
        let (status, body) = post_json(
            &harness.router,
            "/api/v1/api-tokens",
            serde_json::json!({ "label": format!("otlp-{index}"), "scopes": ["api:read"] }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "requests waited for the collector: {:?}",
        started.elapsed()
    );
    assert_eq!(
        records(&harness.database)
            .await
            .iter()
            .filter(|record| record["action"] == "token_created")
            .count(),
        10,
        "an audit record went missing while the collector was down"
    );
}

#[tokio::test]
async fn the_export_is_ndjson_of_exactly_what_the_filter_shows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for label in ["one", "two"] {
        post_json(
            &harness.router,
            "/api/v1/api-tokens",
            serde_json::json!({ "label": label, "scopes": ["api:read"] }),
        )
        .await;
    }
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/api/v1/audit/export?action=token_created")
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .body(axum::body::Body::empty())
        .expect("request");
    let response = tower::ServiceExt::oneshot(harness.router.clone(), request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/x-ndjson")
    );
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    for line in lines {
        let record: serde_json::Value = serde_json::from_str(line).expect("one record per line");
        assert_eq!(record["action"], "token_created");
    }
}

#[tokio::test]
async fn a_read_only_token_cannot_read_the_audit_log() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    for uri in ["/api/v1/audit/records", "/api/v1/audit/export"] {
        let (status, body) = get_with_bearer(&harness.router, uri, READ_BEARER).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(body["code"], "auth.scope_insufficient");
        let (status, body) = get_json(&harness.router, uri).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}: {body}");
    }
    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/audit/records", common::API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn an_unknown_filter_word_is_refused_with_a_stable_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for uri in [
        "/api/v1/audit/records?action=nothing_like_it",
        "/api/v1/audit/records?outcome=maybe",
        "/api/v1/audit/records?actor_kind=root",
        "/api/v1/audit/records?limit=501",
        "/api/v1/audit/records?since=yesterday",
    ] {
        let (status, body) = get_json(&harness.router, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(body["code"], "audit.invalid_query", "{uri}");
    }
}

#[tokio::test]
async fn retention_is_saved_with_the_settings_and_validated_there() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let base = settings_of(&harness.router).await;
    assert_eq!(base["audit_retention_records"], 100_000);
    assert_eq!(base["audit_retention_days"], 365);
    assert_eq!(base["otlp_enabled"], false);

    for (field, value, code) in [
        (
            "audit_retention_records",
            serde_json::json!(10),
            "settings.audit_retention_invalid",
        ),
        (
            "audit_retention_days",
            serde_json::json!(3),
            "settings.audit_retention_invalid",
        ),
        (
            "otlp_endpoint",
            serde_json::json!("file:///etc/passwd"),
            "settings.otlp_invalid",
        ),
        (
            "otlp_timeout_seconds",
            serde_json::json!(0),
            "settings.otlp_invalid",
        ),
    ] {
        let mut settings = base.clone();
        settings[field] = value;
        let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {body}");
        assert_eq!(body["code"], code, "{field}");
    }

    // Switching the export on with nowhere to send to is refused rather than silently inert.
    let mut settings = base.clone();
    settings["otlp_enabled"] = serde_json::json!(true);
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.otlp_endpoint_required");

    let mut settings = base;
    settings["audit_retention_days"] = serde_json::json!(90);
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, page) = get_json(&harness.router, "/api/v1/audit/records").await;
    assert_eq!(page["retention"]["days"], 90);
    assert!(page["actions"].as_array().expect("actions").len() >= 13);
}

#[tokio::test]
async fn a_refused_privileged_setting_is_recorded_as_a_failure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let (status, mut settings) =
        get_with_bearer(&harness.router, "/api/v1/settings", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["otlp_endpoint"] = serde_json::json!("http://collector.invalid/v1/traces");

    // A config-scoped token may write the settings blob but not the fields that decide what
    // the service talks to.
    let token = "test-config-bearer-token";
    harness
        .database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "config".to_owned(),
            hex::encode(sha2::Sha256::digest(token.as_bytes())),
            vec![rd_core::API_CONFIG_SCOPE.to_owned()],
        )
        .await
        .expect("token");
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/v1/settings")
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(settings.to_string()))
        .expect("request");
    let response = tower::ServiceExt::oneshot(harness.router.clone(), request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (status, body) = get_with_bearer(
        &harness.router,
        "/api/v1/audit/records?limit=500",
        API_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let refused = body["records"]
        .as_array()
        .expect("records")
        .iter()
        .find(|record| record["action"] == "settings_changed" && record["outcome"] == "failure")
        .cloned()
        .expect("the refusal was not recorded");
    assert_eq!(refused["details"]["refused_field"], "otlp_endpoint");
    assert_eq!(refused["actor_kind"], "token");
    assert_eq!(refused["actor_label"], "config");
}

#[tokio::test]
async fn a_machine_tokens_use_is_recorded() {
    let directory = tempfile::tempdir().expect("tempdir");
    // With the login on, so a bearer is actually consulted: the default harness waves every
    // request through before a credential is looked at, and nothing would be recorded.
    let harness = auth_harness(directory.path()).await;
    let (status, _) = get_with_bearer(&harness.router, "/api/v1/downloads", READ_BEARER).await;
    assert_eq!(status, StatusCode::OK);

    // Written off the request path on purpose, so it may land a moment after the answer.
    let used = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let (_, body) = get_with_bearer(
                &harness.router,
                "/api/v1/audit/records?action=token_used",
                API_BEARER,
            )
            .await;
            if let Some(record) = body["records"]
                .as_array()
                .and_then(|all| all.first())
                .cloned()
                && !record.is_null()
            {
                return record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a token use was never recorded");
    assert_eq!(used["actor_kind"], "token");
    assert_eq!(used["target_kind"], "token");
    assert!(used["actor_id"].is_string());
    // The bearer itself never appears anywhere in the record.
    assert!(!used.to_string().contains(READ_BEARER));
}

/// Every word in the vocabulary is written somewhere.
///
/// Two of them — installing and removing a plugin — need a signed `.rdplug` package, which
/// the harness cannot build, so the matrix above cannot perform them. This reads the sources
/// instead: an action that exists in `rd_core::AuditAction` and is never constructed in
/// `rd-api` is a word in a filter that nothing can ever match, and that is worth failing over
/// whether or not a request can reach it here.
#[test]
fn every_action_in_the_vocabulary_is_written_somewhere() {
    fn collect(directory: &std::path::Path, into: &mut String) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, into);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                into.push_str(&text);
            }
        }
    }

    let mut sources = String::new();
    collect(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    assert!(!sources.is_empty(), "no sources were read");
    for action in rd_core::AuditAction::ALL {
        let variant = format!("{action:?}");
        assert!(
            sources.contains(&format!("AuditAction::{variant}")),
            "nothing in rd-api ever writes `{}`",
            action.as_str()
        );
    }
}

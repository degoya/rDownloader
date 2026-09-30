//! Stopping the service and the backup before an update over the API (RD-180-02, RD-180-03):
//! only from this machine, the local control token opens these two routes and nothing else,
//! the stop cancels the service's own token, and the backup before an update is checked,
//! reports what it wrote and refuses the update when it cannot be written.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};

use crate::common::{self, Harness};

const CONTROL: &str = "the-local-control-token";
const PASSPHRASE: &str = "correct horse battery staple";

/// A request as the listener delivers it, from `peer`, with `bearer` when given.
fn from_peer(uri: &str, peer: [u8; 4], bearer: Option<&str>, body: &Value) -> Request<Body> {
    let mut builder = common::request_to("POST", uri)
        .header(header::CONTENT_TYPE, "application/json")
        .extension(ConnectInfo(SocketAddr::from((peer, 50_000))));
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

fn data_directory(harness: &Harness) -> std::path::PathBuf {
    harness
        .database_path
        .parent()
        .expect("data directory")
        .to_path_buf()
}

async fn audited(harness: &Harness, action: rd_core::AuditAction) -> Vec<rd_db::AuditRecord> {
    harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

async fn prepare(harness: &Harness, body: Value) -> (StatusCode, Value) {
    common::post_json(&harness.router, "/api/v1/system/update/prepare", body).await
}

#[tokio::test]
async fn a_stop_from_this_machine_cancels_the_service_and_is_audited() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    assert!(!harness.state.shutdown.is_cancelled());
    let (status, body) =
        common::post_json(&harness.router, "/api/v1/system/shutdown", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["stopping"], true);
    assert!(harness.state.shutdown.is_cancelled());
    assert_eq!(
        audited(&harness, rd_core::AuditAction::ServiceStopRequested)
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn neither_route_answers_a_caller_on_another_machine_even_with_an_admin_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    for uri in ["/api/v1/system/shutdown", "/api/v1/system/update/prepare"] {
        let (status, body) = common::send(
            &harness.router,
            from_peer(
                uri,
                [192, 168, 1, 20],
                Some(common::API_BEARER),
                &json!({ "target_version": "9.9.9" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(body["code"], "system.local_only", "{uri}");
    }
    assert!(!harness.state.shutdown.is_cancelled());
    assert!(!rd_backup::pre_update::directory(&data_directory(&harness)).exists());
}

#[tokio::test]
async fn the_local_control_token_opens_its_two_routes_from_this_machine_and_nothing_else() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let state = harness
        .state
        .clone()
        .with_local_control(rd_api::local_control::LocalControl::for_token(CONTROL));
    let router = rd_api::router(state.clone());

    // Another admin route: the token is no credential there.
    let (status, _) = common::get_with_bearer(&router, "/api/v1/system/data-reset", CONTROL).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Its own route from another machine: no credential either.
    let (status, _) = common::send(
        &router,
        from_peer(
            "/api/v1/system/shutdown",
            [192, 168, 1, 20],
            Some(CONTROL),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A wrong token from this machine.
    let (status, _) = common::send(
        &router,
        from_peer(
            "/api/v1/system/shutdown",
            [127, 0, 0, 1],
            Some("guessed"),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!state.shutdown.is_cancelled());

    // The backup before an update and the stop, from loopback with the token.
    let (status, body) = common::send(
        &router,
        from_peer(
            "/api/v1/system/update/prepare",
            [127, 0, 0, 1],
            Some(CONTROL),
            &json!({ "target_version": "9.9.9" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = common::send(
        &router,
        from_peer(
            "/api/v1/system/shutdown",
            [127, 0, 0, 1],
            Some(CONTROL),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(state.shutdown.is_cancelled());
    let stops = audited(&harness, rd_core::AuditAction::ServiceStopRequested).await;
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].actor_kind, rd_core::AuditActorKind::System);
}

#[tokio::test]
async fn without_a_passphrase_the_checked_copy_is_written_and_the_answer_says_why_no_archive() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (status, body) = prepare(
        &harness,
        json!({ "target_version": "9.9.9", "schema_change": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["from_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["target_version"], "9.9.9");
    assert!(body["encrypted_backup"].is_null());
    assert_eq!(body["encrypted_backup_code"], "backup.key_missing");
    let copy = std::path::PathBuf::from(body["database_copy"]["path"].as_str().expect("path"));
    assert!(copy.exists());
    assert!(
        copy.starts_with(rd_backup::pre_update::directory(&data_directory(&harness))),
        "{}",
        copy.display()
    );
    assert!(
        body["database_copy"]["schema_version"]
            .as_i64()
            .expect("schema")
            > 0
    );
    rd_db::snapshot::check_integrity(&copy)
        .await
        .expect("whole");
    assert_eq!(
        audited(&harness, rd_core::AuditAction::UpdatePrepared)
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn with_a_passphrase_the_encrypted_backup_is_written_and_opens_under_its_key() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (status, config) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{config}");
    let (status, body) = prepare(
        &harness,
        json!({ "target_version": "9.9.9", "schema_change": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["encrypted_backup_code"].is_null(), "{body}");
    let archive = &body["encrypted_backup"];
    assert_eq!(archive["key_fingerprint"], config["key_fingerprint"]);
    let path = std::path::PathBuf::from(archive["path"].as_str().expect("path"));
    let key = rd_backup::BackupKey::derive(
        PASSPHRASE,
        rd_backup::stream::read_header(&path).expect("header").salt,
    )
    .await
    .expect("key");
    rd_backup::archive::verify_archive(&path, &key).expect("opens under its key");
    // No staging with an unencrypted copy is left.
    assert!(
        !rd_backup::pre_update::directory(&data_directory(&harness))
            .join("staging")
            .exists()
    );
}

#[tokio::test]
async fn an_encrypted_backup_that_fails_refuses_only_an_update_that_changes_the_schema() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (status, _) = common::put_json(
        &harness.router,
        "/api/v1/backups/passphrase",
        json!({ "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The key is gone from the secret store: nothing can be sealed.
    let reference = harness
        .database
        .backup_config()
        .await
        .expect("config")
        .key
        .expect("key")
        .reference;
    harness.secrets.remove(&reference).await.expect("remove");

    let (status, body) = prepare(
        &harness,
        json!({ "target_version": "9.9.9", "schema_change": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "update.backup_failed");
    assert_eq!(body["params"]["stage"], "encrypted_backup");
    assert_eq!(body["params"]["cause"], "backup.key_unavailable");

    let (status, body) = prepare(
        &harness,
        json!({ "target_version": "9.9.9", "schema_change": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["encrypted_backup"].is_null());
    assert_eq!(body["encrypted_backup_code"], "backup.key_unavailable");
    let failures = audited(&harness, rd_core::AuditAction::UpdatePrepared)
        .await
        .into_iter()
        .filter(|record| record.outcome == rd_core::AuditOutcome::Failure)
        .count();
    assert_eq!(failures, 1);
}

#[tokio::test]
async fn a_copy_that_cannot_be_written_refuses_the_update_and_leaves_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    // A file where the folder has to go: nothing can be written below it.
    let blocked = rd_backup::pre_update::directory(&data_directory(&harness));
    std::fs::write(&blocked, b"in the way").expect("block");
    let (status, body) = prepare(&harness, json!({ "target_version": "9.9.9" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "update.backup_failed");
    assert_eq!(body["params"]["stage"], "database_copy");
    assert_eq!(body["params"]["cause"], "update.copy_failed");
    assert!(blocked.is_file());
}

#[tokio::test]
async fn a_target_version_that_is_no_file_name_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    for version in ["", "../escape", "1.8/0"] {
        let (status, body) = prepare(&harness, json!({ "target_version": version })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{version}: {body}");
        assert_eq!(body["code"], "update.target_version_invalid");
    }
    assert!(!rd_backup::pre_update::directory(&data_directory(&harness)).exists());
}

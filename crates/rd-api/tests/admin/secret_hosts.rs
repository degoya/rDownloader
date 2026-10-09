//! Proxy profiles and remote logins keep their stored credentials to the host they were typed
//! for (RD-1200-06, like object storage profiles since RD-1190-20): a changed scheme, protocol,
//! host or port asks for the password or key again, and every change leaves an audit record.

use axum::http::StatusCode;
use rd_core::AuditAction;
use serde_json::{Value, json};

use crate::common;

const PROXIES: &str = "/api/v1/proxy-profiles";
const LOGINS: &str = "/api/v1/remote-credentials";
const PASSWORD: &str = "proxy-secret-1200-06";
const LOGIN_PASSWORD: &str = "login-secret-1200-06";

fn proxy(endpoint: &str, password: Option<&str>) -> Value {
    json!({
        "name": "Office proxy",
        "kind": "http",
        "endpoint": endpoint,
        "username": "relay",
        "password": password,
    })
}

fn login(protocol: &str, host: &str, port: Option<u16>, secret: Option<&str>) -> Value {
    json!({
        "name": "Archive login",
        "protocol": protocol,
        "host": host,
        "port": port,
        "username": "bob",
        "auth_mode": "password",
        "secret": secret,
    })
}

async fn created(router: &axum::Router, uri: &str, body: Value) -> Value {
    let (status, value) = common::post_json(router, uri, body).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value
}

#[tokio::test]
async fn a_moved_proxy_asks_for_its_password_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let profile = created(
        router,
        PROXIES,
        proxy("http://proxy-a.example:3128", Some(PASSWORD)),
    )
    .await;
    let uri = format!("{PROXIES}/{}", profile["id"].as_str().expect("id"));

    // Before, the stored password went along to whatever proxy the update named.
    for moved in [
        "http://collector.example:3128",
        "http://proxy-a.example:8080",
    ] {
        let (status, body) = common::put_json(router, &uri, proxy(moved, None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{moved}: {body}");
        assert_eq!(
            body["code"], "proxy.password_host_changed",
            "{moved}: {body}"
        );
    }

    // The same proxy keeps it: a rename needs no password.
    let mut renamed = proxy("http://proxy-a.example:3128/", None);
    renamed["name"] = json!("Office proxy 2");
    let (status, body) = common::put_json(router, &uri, renamed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["has_credentials"], true, "{body}");

    // A new proxy with the password typed again, or without a login at all, is a plain change.
    let (status, body) = common::put_json(
        router,
        &uri,
        proxy("http://proxy-b.example:3128", Some(PASSWORD)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["has_credentials"], true, "{body}");
    let mut anonymous = proxy("http://proxy-c.example:3128", None);
    anonymous["username"] = Value::Null;
    let (status, body) = common::put_json(router, &uri, anonymous).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["has_credentials"], false, "{body}");
}

#[tokio::test]
async fn a_moved_remote_login_asks_for_its_password_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let stored = created(
        router,
        LOGINS,
        login("ftps", "files-a.example", None, Some(LOGIN_PASSWORD)),
    )
    .await;
    let uri = format!("{LOGINS}/{}", stored["id"].as_str().expect("id"));

    for (moved, body) in [
        ("host", login("ftps", "collector.example", None, None)),
        ("port", login("ftps", "files-a.example", Some(2121), None)),
        ("protocol", login("ftp", "files-a.example", None, None)),
    ] {
        let (status, body) = common::put_json(router, &uri, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{moved}: {body}");
        assert_eq!(
            body["code"], "remote.secret_host_changed",
            "{moved}: {body}"
        );
    }

    // The same server keeps it: a rename needs no password.
    let mut renamed = login("ftps", "Files-A.example.", Some(21), None);
    renamed["name"] = json!("Archive login 2");
    let (status, body) = common::put_json(router, &uri, renamed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["has_secret"], true, "{body}");

    let (status, body) = common::put_json(
        router,
        &uri,
        login("ftps", "files-b.example", None, Some(LOGIN_PASSWORD)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["host"], "files-b.example", "{body}");
    assert_eq!(body["has_secret"], true, "{body}");
}

async fn changes(harness: &common::Harness, action: AuditAction) -> Vec<rd_db::AuditRecord> {
    harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(action),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
}

#[tokio::test]
async fn every_proxy_and_login_change_leaves_an_audit_record() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let profile = created(
        router,
        PROXIES,
        proxy("http://proxy-a.example:3128", Some(PASSWORD)),
    )
    .await;
    let stored = created(
        router,
        LOGINS,
        login("sftp", "files-a.example", None, Some(LOGIN_PASSWORD)),
    )
    .await;
    for (uri, body) in [
        (
            format!("{PROXIES}/{}", profile["id"].as_str().expect("id")),
            proxy("http://proxy-b.example:3128", Some(PASSWORD)),
        ),
        (
            format!("{LOGINS}/{}", stored["id"].as_str().expect("id")),
            login("sftp", "files-b.example", None, Some(LOGIN_PASSWORD)),
        ),
    ] {
        let (status, value) = common::put_json(router, &uri, body).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let (status, value) = common::delete_json(router, &uri).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let proxies = changes(&harness, AuditAction::ProxyProfileChanged).await;
    let logins = changes(&harness, AuditAction::RemoteCredentialChanged).await;
    for (records, kind, place, value) in [
        (
            &proxies,
            "proxy_profile",
            "endpoint",
            "http://proxy-b.example:3128/",
        ),
        (
            &logins,
            "remote_credential",
            "server",
            "sftp://files-b.example:22",
        ),
    ] {
        // Newest first.
        let kinds: Vec<&str> = records
            .iter()
            .map(|record| record.details["change"].as_str())
            .collect();
        assert_eq!(kinds, ["deleted", "updated", "created"], "{kind}");
        let updated = &records[1];
        assert_eq!(updated.target_kind.as_deref(), Some(kind));
        assert_eq!(updated.details[place], value, "{kind}");
        let fields: Vec<&str> = updated.details["fields"].split(' ').collect();
        assert!(
            fields.contains(&"password")
                && (fields.contains(&"endpoint") || fields.contains(&"host")),
            "{kind}: {fields:?}"
        );
    }
    let all = serde_json::to_string(
        &proxies
            .iter()
            .chain(&logins)
            .map(|record| &record.details)
            .collect::<Vec<_>>(),
    )
    .expect("json");
    assert!(
        !all.contains(PASSWORD) && !all.contains(LOGIN_PASSWORD),
        "a password reached the audit log: {all}"
    );
}

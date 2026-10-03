//! A new administrator password without the current one, from the machine the service runs on
//! (RD-190-24): `POST /api/v1/auth/password/reset`, the route `rdownloader auth reset-password`
//! takes while the service runs.
//!
//! The way in is the point, so most of this file is who may *not* take it: a session, an API
//! token, another machine, and this machine behind a reverse proxy. The way it works is one test
//! of what ends (every session, the limiter's lockout) and what stays (API tokens, second
//! factors), and one of `disable_totp`. The stopped service's way is the command's own test
//! (`crates/rdownloader/src/reset_password_cli_tests.rs`).

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use rd_core::{AuditAction, MfaKind};
use serde_json::json;

use crate::common;

const PASSWORD: &str = "correct-horse-battery";
const NEW: &str = "a-password-set-on-the-host";
const WRONG: &str = "not-the-password-at-all";
const CONTROL: &str = "the-local-control-token";
const ROUTE: &str = "/api/v1/auth/password/reset";

/// The router a service with a local control file answers through.
fn controlled(harness: &common::Harness) -> Router {
    rd_api::router(
        harness
            .state
            .clone()
            .with_local_control(rd_api::local_control::LocalControl::for_token(CONTROL)),
    )
}

/// The command's request with the control token, from `peer`, optionally through a proxy.
fn command(peer: [u8; 4], forwarded_for: Option<&str>, body: &serde_json::Value) -> Request<Body> {
    let mut builder = common::request_to("POST", ROUTE)
        .header(header::AUTHORIZATION, format!("Bearer {CONTROL}"))
        .header(header::CONTENT_TYPE, "application/json")
        .extension(ConnectInfo(std::net::SocketAddr::from((peer, 50_000))));
    if let Some(client) = forwarded_for {
        builder = builder.header("x-forwarded-for", client);
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

async fn signs_in(harness: &common::Harness, password: &str) -> StatusCode {
    common::post_json(
        &harness.router,
        "/api/v1/auth/login",
        json!({ "password": password }),
    )
    .await
    .0
}

async fn resets(harness: &common::Harness) -> Vec<rd_db::AuditRecord> {
    harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(AuditAction::PasswordResetLocal),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
}

/// An authenticator app whose secret is in the vault, and a passkey; returns the app's
/// vault reference.
async fn second_factors(harness: &common::Harness) -> String {
    let reference = harness
        .secrets
        .put_string("JBSWY3DPEHPK3PXP".to_owned())
        .await
        .expect("vault");
    for (kind, material) in [
        (MfaKind::Totp, reference.clone()),
        (MfaKind::Webauthn, "vault://not-read-here".to_owned()),
    ] {
        harness
            .database
            .create_mfa_credential(
                rd_core::MfaCredentialId::new(),
                kind,
                "phone".to_owned(),
                material,
            )
            .await
            .expect("factor");
    }
    reference
}

async fn factor_kinds(harness: &common::Harness) -> Vec<MfaKind> {
    harness
        .database
        .list_mfa_credentials()
        .await
        .expect("factors")
        .into_iter()
        .map(|factor| factor.kind)
        .collect()
}

/// No session, no API token, no other machine, and not this machine behind a proxy: only the
/// command line with the local control token on the service's own machine.
#[tokio::test]
async fn only_the_command_line_on_this_machine_may_reset_the_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let session = common::sign_in(&harness.router, PASSWORD).await;
    let local = controlled(&harness);
    let body = json!({ "new_password": NEW });

    let (status, answer) =
        common::post_json_with_cookie(&local, ROUTE, &session, body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "auth.password_reset_cli_only");
    let (status, answer) =
        common::post_with_bearer(&local, ROUTE, common::API_BEARER, body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "auth.password_reset_cli_only");
    // The control token copied to another machine opens nothing.
    let (status, _) = common::send(&local, command([192, 168, 1, 20], None, &body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Nor on this machine through a reverse proxy, which forwards somebody else's request.
    let (status, _) =
        common::send(&local, command([127, 0, 0, 1], Some("203.0.113.9"), &body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(signs_in(&harness, PASSWORD).await, StatusCode::OK);
    assert_eq!(signs_in(&harness, NEW).await, StatusCode::UNAUTHORIZED);
    assert!(resets(&harness).await.is_empty());
}

/// The reset itself: the new password signs in at once — the lockout the owner ran into is
/// gone — every session has ended, the second factors and API tokens stay, and the record says
/// which way it came without the password.
#[tokio::test]
async fn the_command_line_sets_the_password_and_ends_every_session() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let session = common::sign_in(&harness.router, PASSWORD).await;
    second_factors(&harness).await;
    // The owner who forgot the password has usually locked themselves out trying.
    for _ in 0..6 {
        signs_in(&harness, WRONG).await;
    }
    assert_eq!(
        signs_in(&harness, PASSWORD).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    let local = controlled(&harness);
    let (status, answer) = common::send(
        &local,
        command(
            [127, 0, 0, 1],
            None,
            &json!({ "new_password": NEW, "prompted": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["code"], "auth.password_reset_done");
    assert_eq!(answer["params"]["sessions_ended"], "1");
    assert_eq!(answer["params"]["password_login"], "on");

    let (status, _) = common::get_with_cookie(&harness.router, "/api/v1/sessions", &session).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old session survived");
    assert_eq!(signs_in(&harness, NEW).await, StatusCode::OK);
    assert_eq!(signs_in(&harness, PASSWORD).await, StatusCode::UNAUTHORIZED);
    let (status, _) =
        common::get_with_bearer(&harness.router, "/api/v1/sessions", common::API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "an API token stopped working");
    assert_eq!(factor_kinds(&harness).await.len(), 2);

    let records = resets(&harness).await;
    let [record] = records.as_slice() else {
        panic!("one record expected: {records:?}");
    };
    assert_eq!(record.actor_label.as_deref(), Some("local_control"));
    assert_eq!(record.details["path"], "service");
    assert_eq!(record.details["source"], "prompted");
    assert_eq!(record.details["totp"], "kept");
    let written = serde_json::to_string(record).expect("json");
    assert!(!written.contains(NEW), "the password reached the record");
}

/// For a lost phone as well: the authenticator app and its stored secret go, the passkey stays.
#[tokio::test]
async fn disabling_totp_removes_the_authenticator_app_and_its_secret() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    common::sign_in(&harness.router, PASSWORD).await;
    let reference = second_factors(&harness).await;

    let (status, answer) = common::send(
        &controlled(&harness),
        command(
            [127, 0, 0, 1],
            None,
            &json!({ "new_password": NEW, "disable_totp": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(factor_kinds(&harness).await, [MfaKind::Webauthn]);
    assert!(
        harness.secrets.get(&reference).await.is_err(),
        "the authenticator app's secret is still in the vault"
    );
    assert_eq!(resets(&harness).await[0].details["totp"], "removed");
}

/// The policy holds here as everywhere, and a refused password changes nothing.
#[tokio::test]
async fn the_new_password_meets_the_policy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    common::sign_in(&harness.router, PASSWORD).await;

    let (status, answer) = common::send(
        &controlled(&harness),
        command([127, 0, 0, 1], None, &json!({ "new_password": "short" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "auth.password_too_short");
    assert_eq!(signs_in(&harness, PASSWORD).await, StatusCode::OK);
    assert!(resets(&harness).await.is_empty());
}

//! The OAuth callback with the administrator login switched on (security audit 2026-09-30,
//! finding 5).
//!
//! The provider sends the browser back from its own site, and the session cookie is
//! `SameSite=Strict`, so that navigation carries no cookie. The callback used to cost
//! `api:secrets` and therefore answered every sign-in with a 401. It is public now, and the
//! `state` it echoes is its credential: stored when the flow began, answered once.

use crate::common;

use axum::{body::Body, http::StatusCode};
use common::{auth_harness, sign_in};

const PASSWORD: &str = "correct-horse-battery";
/// What a plugin hands out: a PKCE-sized random value.
const STATE: &str = "q8Jx0nN3vY6tR2wLkZp9HsFe4aUcB7mD1gViOoTyXrE";

/// A redirect flow waiting for its callback, as `begin_oauth` leaves it.
async fn waiting_flow(database: &rd_db::Database) -> rd_core::AccountId {
    let account = database
        .create_account(rd_db::NewAccount {
            provider: "example".to_owned(),
            label: "Example".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    database
        .upsert_auth_flow(rd_db::UpsertAuthFlow {
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-0000000001ff".to_owned(),
            state: rd_core::AuthFlowState::WaitingForUser,
            verification_url: Some("https://accounts.example.com/authorize".to_owned()),
            user_code: None,
            expires_at: None,
            next_poll_at: None,
            message: None,
            token_expires_at: None,
            refresh_ref: None,
            access_ref: None,
            key_ref: None,
            callback_state: Some(STATE.to_owned()),
            flow_state: Some("verifier".to_owned()),
        })
        .await
        .expect("flow");
    account.id
}

/// The provider's redirect, as a browser arriving from the provider's site sends it: no cookie.
async fn callback(router: &axum::Router, state: &str) -> StatusCode {
    let request = common::request_to(
        "GET",
        &format!("/api/v1/oauth/callback?code=provider-code&state={state}"),
    )
    .body(Body::empty())
    .expect("request");
    common::send(router, request).await.0
}

#[tokio::test]
async fn the_callback_arrives_without_a_session_and_its_state_is_answered_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    sign_in(&harness.router, PASSWORD).await;
    let account = waiting_flow(&harness.database).await;

    assert_eq!(
        callback(&harness.router, STATE).await,
        StatusCode::SEE_OTHER
    );

    // Taken: the same state finds nothing now.
    assert!(
        harness
            .database
            .auth_flow_by_callback_state(STATE)
            .await
            .expect("lookup")
            .is_none(),
        "the state survived its callback"
    );
    // No plugin claims the provider here, so the exchange failed -- and the flow says so
    // rather than waiting for a callback that can no longer come.
    let flow = harness
        .database
        .auth_flow(account)
        .await
        .expect("read")
        .expect("flow");
    assert_eq!(flow.state, rd_core::AuthFlowState::Failed);

    // Delivered again, it changes nothing.
    assert_eq!(
        callback(&harness.router, STATE).await,
        StatusCode::SEE_OTHER
    );
    let again = harness
        .database
        .auth_flow(account)
        .await
        .expect("read")
        .expect("flow");
    assert_eq!(again.message, flow.message);
}

#[tokio::test]
async fn a_callback_with_a_state_nobody_issued_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    sign_in(&harness.router, PASSWORD).await;
    let account = waiting_flow(&harness.database).await;

    assert_eq!(
        callback(&harness.router, "a-state-this-service-never-issued-at-all").await,
        StatusCode::SEE_OTHER
    );

    let flow = harness
        .database
        .auth_flow(account)
        .await
        .expect("read")
        .expect("flow");
    assert_eq!(flow.state, rd_core::AuthFlowState::WaitingForUser);
    assert!(
        harness
            .database
            .auth_flow_by_callback_state(STATE)
            .await
            .expect("lookup")
            .is_some(),
        "a stranger's callback spent the real state"
    );
}

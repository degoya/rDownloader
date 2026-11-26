//! Deleting a job at its provider over REST (audit 2026-11-25, TST-7).
//!
//! `POST /api/v1/remote-jobs/{id}/discard` deletes something in a person's account elsewhere, so
//! what is held here is that nothing is asked of a provider without the confirmation, and that a
//! refusal leaves the row as it was. This harness installs no plugin; a provider that deletes and
//! one that refuses are mock providers in `crates/rd-api-core/src/remote_job_service/tests.rs`.

use crate::common;
use crate::remote_jobs_clear::{account, job};

use axum::http::StatusCode;
use rd_core::{RemoteJobId, RemoteJobState};
use serde_json::json;

async fn discard(
    harness: &common::Harness,
    id: RemoteJobId,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    common::post_json(
        &harness.router,
        &format!("/api/v1/remote-jobs/{id}/discard"),
        body,
    )
    .await
}

async fn state_of(harness: &common::Harness, id: RemoteJobId) -> RemoteJobState {
    harness
        .database
        .remote_job(id)
        .await
        .expect("read")
        .expect("the row stays")
        .state
}

#[tokio::test]
async fn an_unconfirmed_discard_is_refused_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let realdebrid = account(&harness, "realdebrid").await;
    let ready = job(&harness, realdebrid, RemoteJobState::Ready, Some("RD-1")).await;

    for body in [json!({}), json!({ "confirmed": false })] {
        let (status, answer) = discard(&harness, ready, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
        assert_eq!(answer["code"], "remote_job.not_confirmed", "{answer}");
    }
    assert_eq!(state_of(&harness, ready).await, RemoteJobState::Ready);
}

#[tokio::test]
async fn a_confirmed_discard_names_why_it_deleted_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let realdebrid = account(&harness, "realdebrid").await;
    let confirmed = || json!({ "confirmed": true });

    let (status, answer) = discard(&harness, RemoteJobId::new(), confirmed()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "remote_job.not_found", "{answer}");

    // The provider never created it: there is nothing to delete there.
    let never_named = job(&harness, realdebrid, RemoteJobState::Failed, None).await;
    let (status, answer) = discard(&harness, never_named, confirmed()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "remote_job.missing_remote_id", "{answer}");
    assert_eq!(
        state_of(&harness, never_named).await,
        RemoteJobState::Failed
    );

    let gone = job(
        &harness,
        realdebrid,
        RemoteJobState::Discarded,
        Some("RD-2"),
    )
    .await;
    let (status, answer) = discard(&harness, gone, confirmed()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "remote_job.already_discarded", "{answer}");

    // No installed plugin runs it: the job stays, under a code that says who has to act.
    let ready = job(&harness, realdebrid, RemoteJobState::Ready, Some("RD-3")).await;
    let (status, answer) = discard(&harness, ready, confirmed()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{answer}");
    assert_eq!(answer["code"], "remote_job.no_plugin", "{answer}");
    assert_eq!(state_of(&harness, ready).await, RemoteJobState::Ready);
}

//! Clearing the remote jobs list over REST (RD-1200-01).
//!
//! `POST /api/v1/remote-jobs/clear` with a provider and state filter: unconfirmed it changes
//! nothing; "here only" removes the filtered rows and leaves a job still running; "at the
//! provider" keeps the row of a job its provider could not delete -- here because no installed
//! plugin runs it, the one failure this harness can produce without a network -- and clears
//! the others all the same. Every clear leaves one audit record. A provider that answers and
//! one that refuses are held against two mock providers in
//! `crates/rd-api-core/src/remote_job_service/clear_tests.rs`.

use crate::common;

use axum::http::StatusCode;
use rd_core::{AccountId, RemoteJobId, RemoteJobSourceKind, RemoteJobState};
use rd_db::{AdvanceRemoteJob, ClaimRemoteJob};
use serde_json::json;

const CLEAR: &str = "/api/v1/remote-jobs/clear";
/// A plugin id nothing installs: a job naming it cannot be deleted at its provider.
const NOT_INSTALLED: &str = "019d0000-0000-7000-8000-0000000012b1";

async fn account(harness: &common::Harness, provider: &str) -> AccountId {
    harness
        .database
        .create_account(rd_db::NewAccount {
            provider: provider.to_owned(),
            label: format!("{provider} clear account"),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

/// A row in `state`, polled no sooner than tomorrow so the sweep leaves it where it is.
async fn job(
    harness: &common::Harness,
    account: AccountId,
    state: RemoteJobState,
    remote_id: Option<&str>,
) -> RemoteJobId {
    let id = RemoteJobId::new();
    harness
        .database
        .claim_remote_job(ClaimRemoteJob {
            id,
            account_id: account,
            plugin_id: NOT_INSTALLED.to_owned(),
            content_key: format!("clear-{id}"),
            source_kind: RemoteJobSourceKind::Magnet,
            source: b"magnet:?xt=urn:btih:remote-jobs-clear".to_vec(),
            source_name: None,
            package_id: None,
        })
        .await
        .expect("claim");
    harness
        .database
        .advance_remote_job(
            id,
            AdvanceRemoteJob {
                state: Some(state),
                remote_id: remote_id.map(str::to_owned),
                next_poll_at: Some(Some(chrono::Utc::now() + chrono::Duration::days(1))),
                ..AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("advance");
    id
}

async fn listed(harness: &common::Harness) -> Vec<String> {
    let (status, body) = common::get_json(&harness.router, "/api/v1/remote-jobs").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut ids: Vec<String> = body
        .as_array()
        .expect("a list")
        .iter()
        .map(|job| job["id"].as_str().expect("an id").to_owned())
        .collect();
    ids.sort();
    ids
}

fn sorted(ids: &[RemoteJobId]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    ids.sort();
    ids
}

/// The audit records the clears left, newest first.
async fn audited(harness: &common::Harness) -> Vec<serde_json::Value> {
    let (status, records) = common::get_json(
        &harness.router,
        "/api/v1/audit/records?action=remote_jobs_cleared&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{records}");
    records["records"].as_array().expect("records").clone()
}

#[tokio::test]
async fn an_unconfirmed_clear_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let realdebrid = account(&harness, "realdebrid").await;
    let ready = job(&harness, realdebrid, RemoteJobState::Ready, None).await;

    let (status, body) = common::post_json(&harness.router, CLEAR, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "remote_job.clear_unconfirmed", "{body}");
    assert_eq!(listed(&harness).await, sorted(&[ready]));
    assert!(audited(&harness).await.is_empty());
}

/// "Clear the list here only", narrowed to one provider and two states: the rows it reaches
/// go, a job still running is named and stays, and the other provider's rows are untouched.
#[tokio::test]
async fn clearing_here_only_follows_the_filter_and_leaves_running_jobs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let realdebrid = account(&harness, "realdebrid").await;
    let torbox = account(&harness, "torbox").await;
    let ready = job(&harness, realdebrid, RemoteJobState::Ready, Some("RD-1")).await;
    let failed = job(&harness, realdebrid, RemoteJobState::Failed, Some("RD-2")).await;
    let discarded = job(
        &harness,
        realdebrid,
        RemoteJobState::Discarded,
        Some("RD-3"),
    )
    .await;
    let working = job(&harness, realdebrid, RemoteJobState::Working, Some("RD-4")).await;
    let other = job(&harness, torbox, RemoteJobState::Failed, Some("TB-1")).await;

    let (status, body) = common::post_json(
        &harness.router,
        CLEAR,
        json!({
            "confirmed": true,
            "provider": "realdebrid",
            "states": ["ready", "failed", "working"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2, "{body}");
    assert_eq!(body["failed"], 0, "{body}");
    let results = body["results"].as_array().expect("results");
    assert!(
        results
            .iter()
            .all(|row| row["removed"] == true && row["provider"] == "realdebrid"),
        "{body}"
    );
    assert_eq!(body["skipped"][0]["id"], working.to_string(), "{body}");
    assert_eq!(body["skipped"].as_array().map(Vec::len), Some(1), "{body}");
    let mut cleared: Vec<String> = results
        .iter()
        .map(|row| row["id"].as_str().expect("an id").to_owned())
        .collect();
    cleared.sort();
    assert_eq!(cleared, sorted(&[ready, failed]));
    assert_eq!(listed(&harness).await, sorted(&[discarded, working, other]));

    let records = audited(&harness).await;
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record["outcome"], "success", "{record}");
    assert_eq!(record["details"]["at_provider"], "false", "{record}");
    assert_eq!(record["details"]["provider"], "realdebrid", "{record}");
    assert_eq!(
        record["details"]["states"], "ready,failed,working",
        "{record}"
    );
    assert_eq!(record["details"]["removed"], "2", "{record}");
    assert_eq!(record["details"]["skipped"], "1", "{record}");
}

/// "Delete at the provider and clear": the job whose provider could not delete it keeps its
/// row and is reported with a code; the job that names nothing at its provider and the one
/// already discarded there are cleared regardless.
#[tokio::test]
async fn a_provider_that_cannot_delete_keeps_its_row_and_the_rest_are_cleared() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let realdebrid = account(&harness, "realdebrid").await;
    let torbox = account(&harness, "torbox").await;
    let never_named = job(&harness, realdebrid, RemoteJobState::Failed, None).await;
    let discarded = job(
        &harness,
        realdebrid,
        RemoteJobState::Discarded,
        Some("RD-1"),
    )
    .await;
    let stuck = job(&harness, torbox, RemoteJobState::Ready, Some("TB-1")).await;

    let (status, body) = common::post_json(
        &harness.router,
        CLEAR,
        json!({ "confirmed": true, "at_provider": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2, "{body}");
    assert_eq!(body["failed"], 1, "{body}");
    let row = body["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|row| row["id"] == stuck.to_string())
        .expect("the stuck job is reported")
        .clone();
    assert_eq!(row["removed"], false, "{row}");
    assert_eq!(row["provider"], "torbox", "{row}");
    assert!(
        row["code"].as_str().is_some_and(|code| !code.is_empty()),
        "{row}"
    );
    assert_eq!(listed(&harness).await, sorted(&[stuck]));
    for cleared in [never_named, discarded] {
        assert!(
            body["results"]
                .as_array()
                .expect("results")
                .iter()
                .any(|row| row["id"] == cleared.to_string() && row["removed"] == true),
            "{cleared}: {body}"
        );
    }

    let records = audited(&harness).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["outcome"], "success", "{}", records[0]);
    assert_eq!(
        records[0]["details"]["at_provider"], "true",
        "{}",
        records[0]
    );
    assert_eq!(records[0]["details"]["failed"], "1", "{}", records[0]);

    // Asked again, only the stuck job is left and nothing at all goes: a failure, on record.
    let (status, body) = common::post_json(
        &harness.router,
        CLEAR,
        json!({ "confirmed": true, "at_provider": true, "provider": "torbox" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["removed"].clone(), body["failed"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(audited(&harness).await[0]["outcome"], "failure");
}

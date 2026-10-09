//! `POST /api/v1/remote-jobs/clear`: the whole (filtered) list in one request (RD-1200-01).
//!
//! The selection is a filter -- a provider, a set of states -- and not a list of ids, so the
//! request means what the person looked at even when a row moved in between. Per row it is the
//! same as the two single-job requests; `RemoteJobService::clear` says how.

use axum::{Json, extract::State};
use rd_core::{RemoteJobId, RemoteJobState};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    AppState,
    error::ApiError,
    remote_job_service::{ClearedRemoteJob, RemoteJobClearFilter},
};

/// Which remote jobs to clear, and whether at their provider too.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct RemoteJobClearRequest {
    /// Has to be `true`. Absent or `false`, nothing happens and the request is refused under
    /// `remote_job.clear_unconfirmed`.
    #[serde(default)]
    pub confirmed: bool,
    /// `true` deletes each job at its provider before its row goes (a job that names nothing
    /// there only loses its row); `false` removes the rows and sends nothing anywhere.
    #[serde(default)]
    pub at_provider: bool,
    /// Only the jobs of accounts at this provider (its slug). Absent: every provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Only jobs in these states. Empty: every state. A job still running (`submitting`,
    /// `preparing`, `working`) is left out either way and listed under `skipped`.
    #[serde(default)]
    pub states: Vec<RemoteJobState>,
}

/// What became of one remote job.
#[derive(Debug, Serialize, ToSchema)]
pub struct RemoteJobClearItem {
    pub id: RemoteJobId,
    /// The provider of the job's account; absent when the account is gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Whether the row went. `false` with `code` and `message` when it stayed.
    pub removed: bool,
    /// Why the row stayed: the provider's own code, or `remote_job.clear_failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The English fallback for `code`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl From<ClearedRemoteJob> for RemoteJobClearItem {
    fn from(job: ClearedRemoteJob) -> Self {
        let (code, message) = job
            .refusal
            .map(|refusal| (refusal.code, refusal.message))
            .unzip();
        Self {
            id: job.id,
            provider: job.provider,
            removed: code.is_none(),
            code,
            message,
        }
    }
}

/// What a clear did, job by job.
#[derive(Debug, Serialize, ToSchema)]
pub struct RemoteJobClearResponse {
    /// How many rows went.
    pub removed: usize,
    /// How many rows stayed because their provider refused or could not be reached.
    pub failed: usize,
    /// Every job the filter reached that was not still running, in list order.
    pub results: Vec<RemoteJobClearItem>,
    /// The jobs the filter reached that were still running and were left alone.
    pub skipped: Vec<RemoteJobClearItem>,
}

/// Clears the remote jobs list, or the part a provider and state filter selects.
///
/// Jobs still running at the provider are left out and listed. With `at_provider` each job is
/// deleted at its provider first; a provider that refuses keeps that job's row, and every other
/// job is cleared all the same. One audit record says what was asked and how it ended.
#[utoipa::path(post, path = "/api/v1/remote-jobs/clear", tag = "configuration", request_body = RemoteJobClearRequest, responses((status = 200, body = RemoteJobClearResponse), (status = 400, description = "remote_job.clear_unconfirmed")))]
pub async fn clear_remote_jobs(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<RemoteJobClearRequest>,
) -> Result<Json<RemoteJobClearResponse>, ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "remote_job.clear_unconfirmed",
            "clearing the remote jobs list has to be confirmed explicitly",
        ));
    }
    let filter = RemoteJobClearFilter {
        provider: request.provider,
        states: request.states,
    };
    let report = state
        .remote_jobs
        .clear(&filter, request.at_provider)
        .await?;
    let (removed, failed) = (report.removed(), report.failed());
    let states = if filter.states.is_empty() {
        "all".to_owned()
    } else {
        filter
            .states
            .iter()
            .map(|state| state.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    // A clear that removed nothing because every provider refused is a failure; one that
    // removed some rows succeeded, and its record says how many stayed.
    let event = if removed == 0 && failed > 0 {
        crate::audit::AuditEvent::failure(rd_core::AuditAction::RemoteJobsCleared)
    } else {
        crate::audit::AuditEvent::success(rd_core::AuditAction::RemoteJobsCleared)
    };
    crate::audit::record(
        &state,
        event
            .by(&audit)
            .target("remote_jobs", "list")
            .detail("at_provider", request.at_provider)
            .detail(
                "provider",
                filter
                    .provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty())
                    .unwrap_or("all"),
            )
            .detail("states", states)
            .detail("removed", removed)
            .detail("failed", failed)
            .detail("skipped", report.skipped.len()),
    )
    .await;
    Ok(Json(RemoteJobClearResponse {
        removed,
        failed,
        results: report.results.into_iter().map(Into::into).collect(),
        skipped: report.skipped.into_iter().map(Into::into).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use rd_core::RemoteJobId;

    use super::{RemoteJobClearItem, RemoteJobClearRequest};
    use crate::remote_job_service::{ClearedRemoteJob, RemoteJobRefused};

    /// A body that says nothing clears nothing, and nothing at any provider.
    #[test]
    fn an_empty_body_is_neither_confirmed_nor_at_the_provider() {
        let empty: RemoteJobClearRequest = serde_json::from_str("{}").expect("empty body");
        assert!(!empty.confirmed);
        assert!(!empty.at_provider);
        assert!(empty.provider.is_none() && empty.states.is_empty());
        let filtered: RemoteJobClearRequest = serde_json::from_str(
            r#"{"confirmed":true,"provider":"torbox","states":["failed","ready"]}"#,
        )
        .expect("filtered body");
        assert_eq!(filtered.states.len(), 2);
    }

    #[test]
    fn a_refused_row_carries_its_code_and_a_cleared_one_none() {
        let id = RemoteJobId::new();
        let refused = RemoteJobClearItem::from(ClearedRemoteJob {
            id,
            provider: Some("torbox".to_owned()),
            refusal: Some(RemoteJobRefused {
                code: "torbox.offline".to_owned(),
                message: "the provider did not answer".to_owned(),
            }),
        });
        assert!(!refused.removed);
        assert_eq!(refused.code.as_deref(), Some("torbox.offline"));
        let cleared = RemoteJobClearItem::from(ClearedRemoteJob {
            id,
            provider: None,
            refusal: None,
        });
        assert!(cleared.removed && cleared.code.is_none() && cleared.message.is_none());
    }
}

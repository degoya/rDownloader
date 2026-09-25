//! Emptying the log store, the audit log and the transfer statistics (RD-120-34), and the
//! notification history (RD-130-08).
//!
//! Separate actions and deliberately none that does them all. Somebody testing wants to start
//! a run from an empty log while keeping the statistics that say how the last week went, and a
//! single "reset everything" button cannot express that. The notification history's button
//! sits at the history itself rather than in the system settings, but it is the same action
//! with the same confirmation, so it lives here.
//!
//! ## Why the confirmation is a value and not only a dialog
//!
//! Every one of these requests carries `confirmed: true` and is refused without it, the same
//! way `SettingsRemoteJobsCard` sends one (`design.md`). The dialog is the person's decision;
//! the flag is what makes a client that never drew a dialog — an MCP caller, a script — say
//! the same thing out loud. It is also what lets these be offered over MCP at all: the
//! toolbox leaves out capabilities that destroy something without a confirmation, and an
//! explicit argument is a confirmation the caller had to supply.
//!
//! ## Why the count comes before the question
//!
//! [`data_reset_preview`] exists so the dialog can name what goes. "Delete 41,208 log
//! records" is a question somebody can answer; "delete the logs" is one they answer by habit.
//!
//! ## What is never touched
//!
//! Downloads, packages, candidates, categories, accounts and the settings document. Each
//! action deletes only from the store it names, and `crates/rd-api/tests/data_reset.rs` holds
//! that line with a queue that survives all of them. Within the notification history, a
//! delivery still queued or retrying is never touched either: it is a notification the worker
//! has yet to send, not a record of one.

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, audit::AuditContext, error::ApiError};

/// The one field every clear request carries.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct DataClearRequest {
    /// `true`, or the request is refused with `data_reset.not_confirmed`.
    #[serde(default)]
    pub confirmed: bool,
}

/// What one clear removed.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
pub struct DataClearResponse {
    /// How many records went. Zero is a success, not a failure: an empty store was already
    /// what the caller asked for.
    pub removed: u64,
}

/// How much each store holds right now, for the question the dialog asks.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
pub struct DataResetPreview {
    /// Records in the service log.
    pub logs: u64,
    /// Records in the audit log.
    pub audit: u64,
    /// Rows in the transfer statistics: the buckets and the all-time totals together.
    pub stats: u64,
    /// Deliveries in the notification history a clear would remove: every one except those
    /// still queued or retrying (RD-130-08).
    pub notifications: u64,
}

/// `400` when a clear arrives without its confirmation flag.
fn not_confirmed(target: &str) -> ApiError {
    ApiError::bad_request(
        "data_reset.not_confirmed",
        "This clears data for good and must be confirmed",
    )
    .with_param("target", target)
}

fn confirm(request: &DataClearRequest, target: &str) -> Result<(), ApiError> {
    if request.confirmed {
        Ok(())
    } else {
        Err(not_confirmed(target))
    }
}

/// How many records each store holds, so a confirmation can name the number.
#[utoipa::path(
    get,
    path = "/api/v1/system/data-reset",
    tag = "system",
    responses((status = 200, body = DataResetPreview))
)]
pub async fn data_reset_preview(
    State(state): State<AppState>,
) -> Result<Json<DataResetPreview>, ApiError> {
    Ok(Json(DataResetPreview {
        logs: state.database.count_log_records().await?,
        audit: state.database.count_audit_records().await?,
        stats: state.database.count_transfer_stats().await?,
        notifications: state
            .database
            .count_clearable_notification_deliveries()
            .await?,
    }))
}

/// Empties the service log. Nothing else in the installation changes.
#[utoipa::path(
    post,
    path = "/api/v1/diagnostics/logs/clear",
    tag = "diagnostics",
    request_body = DataClearRequest,
    responses(
        (status = 200, body = DataClearResponse),
        (status = 400, description = "data_reset.not_confirmed"),
    )
)]
pub async fn clear_log_records(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<DataClearRequest>,
) -> Result<Json<DataClearResponse>, ApiError> {
    confirm(&request, "logs")?;
    let removed = state.database.clear_log_records().await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::LogsCleared)
            .by(&audit)
            .target("logs", "log_records")
            .detail(rd_db::CLEARED_DETAIL_KEY, removed),
    )
    .await;
    Ok(Json(DataClearResponse { removed }))
}

/// Empties the audit log and writes that act into it as the first new entry.
///
/// The entry is not written afterwards by this handler: it travels down with the command and
/// commits in the same transaction as the delete (`rd_db::Database::clear_audit_records`).
/// An audit log that is empty with nothing in it saying who emptied it and when has lost the
/// one trace that explains why it starts where it does, and that is the condition on which
/// this action is offered at all.
#[utoipa::path(
    post,
    path = "/api/v1/audit/records/clear",
    tag = "audit",
    request_body = DataClearRequest,
    responses(
        (status = 200, body = DataClearResponse),
        (status = 400, description = "data_reset.not_confirmed"),
    )
)]
pub async fn clear_audit_records(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<DataClearRequest>,
) -> Result<Json<DataClearResponse>, ApiError> {
    confirm(&request, "audit")?;
    let record = crate::audit::to_record(
        crate::audit::AuditEvent::success(rd_core::AuditAction::AuditCleared)
            .by(&audit)
            .target("audit", "audit_records"),
    );
    let removed = state.database.clear_audit_records(record).await?;
    Ok(Json(DataClearResponse { removed }))
}

/// Empties the transfer statistics: the buckets behind the charts and the all-time totals.
#[utoipa::path(
    post,
    path = "/api/v1/stats/transfers/clear",
    tag = "stats",
    request_body = DataClearRequest,
    responses(
        (status = 200, body = DataClearResponse),
        (status = 400, description = "data_reset.not_confirmed"),
    )
)]
pub async fn clear_transfer_stats(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<DataClearRequest>,
) -> Result<Json<DataClearResponse>, ApiError> {
    confirm(&request, "stats")?;
    let removed = state.database.clear_transfer_stats().await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::StatsCleared)
            .by(&audit)
            .target("stats", "transfer_stats")
            .detail(rd_db::CLEARED_DETAIL_KEY, removed),
    )
    .await;
    Ok(Json(DataClearResponse { removed }))
}

/// Empties the notification history (RD-130-08).
///
/// A delivery still `queued` or `retrying` stays, by the rule the history's own per-rule trim
/// follows (`rd_db::notify_store::queue_delivery`): the worker owes it an attempt, and removing
/// it would silently drop the notification rather than only its record. What stays is
/// therefore not an empty list but the work still in flight, and `removed` counts only what
/// went.
#[utoipa::path(
    post,
    path = "/api/v1/notifications/deliveries/clear",
    tag = "notifications",
    request_body = DataClearRequest,
    responses(
        (status = 200, body = DataClearResponse),
        (status = 400, description = "data_reset.not_confirmed"),
    )
)]
pub async fn clear_notification_deliveries(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<DataClearRequest>,
) -> Result<Json<DataClearResponse>, ApiError> {
    confirm(&request, "notifications")?;
    let removed = state.database.clear_notification_deliveries().await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::NotificationsCleared)
            .by(&audit)
            .target("notifications", "notification_deliveries")
            .detail(rd_db::CLEARED_DETAIL_KEY, removed),
    )
    .await;
    Ok(Json(DataClearResponse { removed }))
}

#[cfg(test)]
mod tests {
    use super::{DataClearRequest, confirm};

    #[test]
    fn an_unconfirmed_clear_is_refused_with_a_stable_code() {
        let error = confirm(&DataClearRequest { confirmed: false }, "logs")
            .expect_err("an unconfirmed clear is refused");
        assert_eq!(error.code(), "data_reset.not_confirmed");
    }

    #[test]
    fn a_confirmed_clear_passes() {
        assert!(confirm(&DataClearRequest { confirmed: true }, "logs").is_ok());
    }
}

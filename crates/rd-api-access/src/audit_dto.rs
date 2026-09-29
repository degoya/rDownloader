//! Wire types of the audit log (RD-110-03).

use std::collections::BTreeMap;

use rd_core::{AuditAction, AuditActorKind, AuditOutcome};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Records one read returns at most; the default when `limit` is absent is 200.
pub const MAX_AUDIT_PAGE: u32 = 500;
pub const DEFAULT_AUDIT_PAGE: u32 = 200;
/// Records one export writes at most.
///
/// Ten thousand rows of NDJSON is a file a person can open; the point of the export is to
/// take a window of the log elsewhere, not to copy the database. A larger window is asked for
/// by narrowing the filter, which is also what makes the file worth reading.
pub const MAX_AUDIT_EXPORT: u32 = 10_000;

/// The filters of `GET /api/v1/audit/records` and `GET /api/v1/audit/export`.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct AuditQueryParams {
    /// One action word, such as `login_failed`.
    pub action: Option<String>,
    /// `success` or `failure`.
    pub outcome: Option<String>,
    /// `session`, `token`, `anonymous` or `system`.
    pub actor_kind: Option<String>,
    /// An actor id, exactly.
    pub actor_id: Option<String>,
    /// A target family, such as `download`.
    pub target_kind: Option<String>,
    /// A target id, exactly.
    pub target_id: Option<String>,
    /// A trace id, exactly; the same value the log viewer filters on.
    pub trace_id: Option<String>,
    /// RFC 3339; records at or after this moment.
    pub since: Option<String>,
    /// RFC 3339; records at or before this moment.
    pub until: Option<String>,
    /// Records older than this id, for paging backwards.
    pub before_id: Option<i64>,
    /// Newest rows to return (1-500; the export allows up to 10000).
    pub limit: Option<u32>,
}

/// One stored audit record. There is no field here that could hold a credential.
#[derive(Serialize, ToSchema)]
pub struct AuditRecordResponse {
    pub id: i64,
    /// RFC 3339, UTC, milliseconds.
    pub recorded_at: String,
    pub action: AuditAction,
    pub outcome: AuditOutcome,
    pub actor_kind: AuditActorKind,
    pub actor_id: Option<String>,
    pub actor_label: Option<String>,
    pub client_address: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    pub trace_id: Option<String>,
    pub details: BTreeMap<String, String>,
}

/// What retention keeps, as configured.
#[derive(Serialize, ToSchema)]
pub struct AuditRetentionResponse {
    pub records: u32,
    pub days: u32,
}

/// A page of the audit log, newest first.
#[derive(Serialize, ToSchema)]
pub struct AuditRecordsResponse {
    pub records: Vec<AuditRecordResponse>,
    /// True when the page is full, so a client knows there may be more behind `before_id`.
    pub full_page: bool,
    /// Records the whole log holds, whatever the filter.
    pub total: u64,
    pub retention: AuditRetentionResponse,
    /// Every action word the service can write, so a filter can be built without a hardcoded
    /// list in the frontend.
    pub actions: Vec<AuditAction>,
}

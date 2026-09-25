//! Wire types of the log viewer and the diagnostic bundle (RD-110-02).

use std::collections::BTreeMap;

use rd_core::LogLevel;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Records one read returns at most; the default when `limit` is absent is 200.
pub const MAX_LOG_PAGE: u32 = 500;
pub const DEFAULT_LOG_PAGE: u32 = 200;

/// The filters of `GET /api/v1/diagnostics/logs`. Every one is optional.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct LogQueryParams {
    /// This level and the more severe ones: `trace`, `debug`, `info`, `warn` or `error`.
    pub level: Option<String>,
    /// A component prefix, such as `rd_http`.
    pub component: Option<String>,
    /// A stable code, exactly.
    pub code: Option<String>,
    /// A correlation id, exactly.
    pub correlation_id: Option<String>,
    /// A case-insensitive substring of the message.
    pub search: Option<String>,
    /// RFC 3339; records at or after this moment.
    pub since: Option<String>,
    /// RFC 3339; records at or before this moment.
    pub until: Option<String>,
    /// Records older than this id, for paging backwards.
    pub before_id: Option<i64>,
    /// Newest rows to return (1-500).
    pub limit: Option<u32>,
}

/// One stored record, as the layer redacted it.
#[derive(Serialize, ToSchema)]
pub struct LogRecordResponse {
    pub id: i64,
    /// RFC 3339, UTC, milliseconds.
    pub recorded_at: String,
    pub level: LogLevel,
    pub component: String,
    pub code: Option<String>,
    pub correlation_id: Option<String>,
    pub message: String,
    pub fields: BTreeMap<String, String>,
}

/// What retention keeps, as configured.
#[derive(Serialize, ToSchema)]
pub struct LogRetentionResponse {
    pub records: u32,
    pub days: u32,
}

/// A page of the log store, newest first.
#[derive(Serialize, ToSchema)]
pub struct LogRecordsResponse {
    pub records: Vec<LogRecordResponse>,
    /// True when the page is full, so a client knows there may be more behind `before_id`.
    pub full_page: bool,
    /// Records the whole store holds, whatever the filter.
    pub total: u64,
    /// Records the capture layer handed over since the process started.
    pub captured: u64,
    /// Records the capture layer dropped because the sink was behind.
    pub dropped: u64,
    pub retention: LogRetentionResponse,
}

/// The preview a person approves before a bundle is written.
#[derive(Serialize, ToSchema)]
pub struct BundlePreviewResponse {
    pub entries: Vec<rd_diagnostics::InventoryEntry>,
    /// What no bundle ever contains, stated so the approval is informed. Codes the client
    /// translates, each with the English the archive keeps (RD-120-15).
    pub excluded: Vec<rd_diagnostics::Note>,
    /// The inventory digest the approval quotes back.
    pub digest: String,
    /// Where the archive will be written.
    pub directory: String,
}

/// The approval: the digest from the preview, and which of its entries to include.
#[derive(Deserialize, ToSchema)]
pub struct CreateBundleRequest {
    /// Must be `true`; the server refuses anything else.
    pub approved: bool,
    /// The `digest` of the preview the person saw.
    pub digest: String,
    /// Entry ids from the preview. An unknown id is refused.
    pub entries: Vec<String>,
}

/// The archive that was written.
#[derive(Serialize, ToSchema)]
pub struct BundleCreatedResponse {
    pub file_name: String,
    pub path: String,
    pub bytes: u64,
    pub manifest: rd_diagnostics::Manifest,
}

//! Component schemas of the statistics and diagnostics areas, apart from the shared ones in
//! `schemas.rs` so neither file outgrows the 500-line rule.

use utoipa::OpenApi;

use crate::{diagnostics_dto, stats_handlers};

/// Transfer statistics, the log store and the diagnostic bundle.
#[derive(OpenApi)]
#[openapi(components(schemas(
    stats_handlers::StatsRange,
    stats_handlers::TransferStatsFigures,
    stats_handlers::TransferStatsBucket,
    stats_handlers::TransferStatsGroup,
    stats_handlers::TransferStatsResponse,
    stats_handlers::StatsBucketWidth,
    stats_handlers::UsenetServerTrafficEntry,
    stats_handlers::UsenetTrafficResponse,
    diagnostics_dto::LogRecordResponse,
    diagnostics_dto::LogRetentionResponse,
    diagnostics_dto::LogRecordsResponse,
    diagnostics_dto::BundlePreviewResponse,
    diagnostics_dto::CreateBundleRequest,
    diagnostics_dto::BundleCreatedResponse,
    rd_core::LogLevel,
    rd_diagnostics::Inventory,
    rd_diagnostics::InventoryEntry,
    rd_diagnostics::Note,
    rd_diagnostics::Manifest,
    rd_diagnostics::bundle::ManifestEntry,
    rd_diagnostics::Check,
    rd_diagnostics::CheckStatus,
)))]
pub(crate) struct DiagnosticsSchemas;

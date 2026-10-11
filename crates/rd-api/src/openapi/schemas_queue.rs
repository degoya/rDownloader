//! Component schemas of the queue's package export (RD-1210-01) and re-resolve (RD-1210-01,
//! RD-1250-05), apart from the shared ones in `schemas.rs` so neither file outgrows the 500-line
//! rule.

use utoipa::OpenApi;

use crate::dto;

/// The package export request and its formats; the re-resolve request and its answer.
#[derive(OpenApi)]
#[openapi(components(schemas(
    dto::PackageExportRequest,
    dto::PackageExportFormat,
    dto::ReresolveRequest,
    dto::ReresolveResponse,
    dto::ReresolveResult,
)))]
pub(crate) struct QueueSchemas;

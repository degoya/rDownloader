//! The log viewer's read and the diagnostic bundle over REST (RD-110-02).
//!
//! Nothing here redacts: the store only ever received what the capture layer had already
//! redacted, and the configuration is scrubbed inside `rd_diagnostics::bundle` where the rule
//! has its test. What this module decides is *when* a bundle may be written — after a preview,
//! and only for the inventory the person saw — and where.

use std::path::PathBuf;

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use rd_core::{LogLevel, LogRetentionSettings};
use rd_db::LogQuery;
use rd_diagnostics::{BundleInput, bundle};

use crate::{
    AppState,
    diagnostics_checks::{SystemChecksInput, system_checks},
    diagnostics_dto::{
        BundleCreatedResponse, BundlePreviewResponse, CreateBundleRequest, DEFAULT_LOG_PAGE,
        LogQueryParams, LogRecordResponse, LogRecordsResponse, LogRetentionResponse, MAX_LOG_PAGE,
    },
    error::ApiError,
};

/// The directory bundles are written to: `diagnostics/` inside the data directory.
///
/// Refused rather than defaulted when the binary registered no data directory, because a
/// relative `data/` would put the archive wherever the process happens to run — and a
/// diagnostic bundle whose location is a guess is not something to hand a person.
fn bundle_directory() -> Result<PathBuf, ApiError> {
    rd_core::data_directory()
        .map(|directory| directory.join("diagnostics"))
        .ok_or_else(|| {
            ApiError::conflict(
                "diagnostics.no_data_directory",
                "The service has no data directory to write a bundle into",
            )
        })
}

fn parse_moment(value: Option<&str>, name: &str) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(text) = value.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(text)
        .map(|moment| Some(moment.with_timezone(&Utc)))
        .map_err(|_| {
            ApiError::bad_request(
                "diagnostics.invalid_query",
                "A log filter value could not be read",
            )
            .with_param("field", name)
        })
}

/// Turns the query string into the store's query, refusing what it cannot mean.
fn to_query(params: &LogQueryParams) -> Result<LogQuery, ApiError> {
    let min_level = match params
        .level
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(word) => Some(LogLevel::parse(word).ok_or_else(|| {
            ApiError::bad_request("diagnostics.invalid_query", "Unknown log level")
                .with_param("field", "level")
        })?),
        None => None,
    };
    let limit = params.limit.unwrap_or(DEFAULT_LOG_PAGE);
    if limit == 0 || limit > MAX_LOG_PAGE {
        return Err(ApiError::bad_request(
            "diagnostics.invalid_query",
            "The page size must be between 1 and 500",
        )
        .with_param("field", "limit"));
    }
    Ok(LogQuery {
        min_level,
        component: params.component.clone(),
        code: params.code.clone(),
        correlation_id: params.correlation_id.clone(),
        search: params.search.clone(),
        since: parse_moment(params.since.as_deref(), "since")?,
        until: parse_moment(params.until.as_deref(), "until")?,
        before_id: params.before_id,
        limit,
    })
}

fn to_response(record: rd_db::LogRecord) -> LogRecordResponse {
    LogRecordResponse {
        id: record.id,
        recorded_at: record
            .recorded_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        level: record.level,
        component: record.component,
        code: record.code,
        correlation_id: record.correlation_id,
        message: record.message,
        fields: record.fields,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/diagnostics/logs",
    tag = "diagnostics",
    params(LogQueryParams),
    responses((status = 200, body = LogRecordsResponse))
)]
pub async fn list_log_records(
    State(state): State<AppState>,
    Query(params): Query<LogQueryParams>,
) -> Result<Json<LogRecordsResponse>, ApiError> {
    let query = to_query(&params)?;
    let records = state.database.query_log_records(&query).await?;
    let total = state.database.count_log_records().await?;
    // Falls back to defaults rather than refusing: this only labels the page, and the
    // accessor reports a malformed blob.
    let retention: LogRetentionSettings = state.database.service_settings_or_default().await?;
    let capture = rd_diagnostics::snapshot();
    let full_page = records.len() as u32 >= query.limit;
    Ok(Json(LogRecordsResponse {
        records: records.into_iter().map(to_response).collect(),
        full_page,
        total,
        captured: capture.captured,
        dropped: capture.dropped,
        retention: LogRetentionResponse {
            records: retention.log_retention_records,
            days: retention.log_retention_days,
        },
    }))
}

/// Everything a bundle is built from, read fresh: the preview and the approval both call
/// this, and the digest comparison between them is what proves the person saw this state.
async fn collect_input(state: &AppState) -> Result<BundleInput, ApiError> {
    let settings = crate::handlers::stored_settings(&state.database).await?;
    let configuration = serde_json::to_value(&settings).map_err(anyhow::Error::new)?;
    let plugins = state
        .plugins
        .list_installed()
        .await?
        .into_iter()
        .map(|manifest| (manifest.id.to_string(), manifest.version))
        .collect();
    let checks = system_checks(&SystemChecksInput {
        vendor_directory: settings.vendor_directory.clone(),
        trusted_proxies: settings.trusted_proxies.clone(),
        external_url: settings.external_url.clone(),
        cookie_security: settings.cookie_security,
    })
    .await;
    let recent_errors = state
        .database
        .query_log_records(&LogQuery {
            min_level: Some(LogLevel::Warn),
            limit: bundle::RECENT_ERRORS_LIMIT as u32,
            ..LogQuery::default()
        })
        .await?;
    let log_records_total = state.database.count_log_records().await?;
    Ok(BundleInput {
        application_version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        plugins,
        configuration,
        checks,
        recent_errors,
        log_records_total,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/diagnostics/bundle/preview",
    tag = "diagnostics",
    responses((status = 200, body = BundlePreviewResponse))
)]
pub async fn preview_diagnostic_bundle(
    State(state): State<AppState>,
) -> Result<Json<BundlePreviewResponse>, ApiError> {
    let directory = bundle_directory()?;
    let input = collect_input(&state).await?;
    let inventory = bundle::inventory(&input)?;
    Ok(Json(BundlePreviewResponse {
        entries: inventory.entries,
        excluded: inventory.excluded,
        digest: inventory.digest,
        directory: directory.display().to_string(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/diagnostics/bundle",
    tag = "diagnostics",
    request_body = CreateBundleRequest,
    responses(
        (status = 200, body = BundleCreatedResponse),
        (status = 400, description = "diagnostics.approval_required"),
        (status = 409, description = "diagnostics.preview_stale")
    )
)]
pub async fn create_diagnostic_bundle(
    State(state): State<AppState>,
    Json(request): Json<CreateBundleRequest>,
) -> Result<Json<BundleCreatedResponse>, ApiError> {
    if !request.approved {
        return Err(ApiError::bad_request(
            "diagnostics.approval_required",
            "A diagnostic bundle is only written after its preview was approved",
        ));
    }
    let directory = bundle_directory()?;
    let input = collect_input(&state).await?;
    let inventory = bundle::inventory(&input)?;
    if inventory.digest != request.digest {
        return Err(ApiError::conflict(
            "diagnostics.preview_stale",
            "The inventory changed since the preview; review it again",
        ));
    }
    if request.entries.is_empty() {
        return Err(ApiError::bad_request(
            "diagnostics.approval_required",
            "Select at least one entry to include",
        ));
    }
    let built = bundle::build(&input, &request.entries, Utc::now()).map_err(|error| {
        ApiError::bad_request("diagnostics.invalid_query", error.to_string())
            .with_param("field", "entries")
    })?;
    let file_name = bundle::file_name(Utc::now());
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(anyhow::Error::new)?;
    let path = directory.join(&file_name);
    tokio::fs::write(&path, &built.bytes)
        .await
        .map_err(anyhow::Error::new)?;
    tracing::info!(file = %file_name, entries = built.manifest.entries.len(), "diagnostic bundle written");
    Ok(Json(BundleCreatedResponse {
        file_name,
        path: path.display().to_string(),
        bytes: built.bytes.len() as u64,
        manifest: built.manifest,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/diagnostics/bundles/{name}",
    tag = "diagnostics",
    params(("name" = String, Path, description = "A bundle file name as `POST /api/v1/diagnostics/bundle` returned it")),
    responses(
        (status = 200, description = "The archive", body = Vec<u8>, content_type = "application/zip"),
        (status = 404, description = "diagnostics.bundle_not_found")
    )
)]
pub async fn download_diagnostic_bundle(Path(name): Path<String>) -> Result<Response, ApiError> {
    let not_found = || {
        ApiError::not_found(
            "diagnostics.bundle_not_found",
            "No diagnostic bundle of that name",
        )
    };
    // Only a name this service generated is ever opened: there is no separator, no `..` and
    // no other file in that directory the pattern could match.
    if !bundle::is_file_name(&name) {
        return Err(not_found());
    }
    let path = bundle_directory()?.join(&name);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(not_found()),
        Err(error) => return Err(anyhow::Error::new(error).into()),
    };
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .map_err(anyhow::Error::new)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(bytes),
    )
        .into_response())
}

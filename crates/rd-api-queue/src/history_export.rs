//! The download history as a file (RD-1240-14): CSV for a spreadsheet, NDJSON for a script.
//!
//! The audit export is the pattern: the same filters as the list, so the file holds what the
//! list shows, newest first and cut at [`MAX_HISTORY_EXPORT`] entries; `X-Total-Count` says how
//! many the filters matched, so a cut file can be told from a whole one. Unlike an audit record a
//! history entry is flat, so CSV loses nothing but the failure's parameters, which only
//! translate its code; NDJSON keeps every field as the list returns it.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{SecondsFormat, Utc};
use rd_core::HistoryEntry;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    ApiError, AppState,
    history_handlers::{HistoryFilter, HistoryFilterQuery},
};

/// The most entries one export holds; the newest of what the filters match.
pub const MAX_HISTORY_EXPORT: u64 = 10_000;

/// The two file formats of the history export.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HistoryExportFormat {
    /// Comma-separated values with a header row, UTF-8 with a byte order mark.
    #[default]
    Csv,
    /// One JSON entry per line, as `GET /api/v1/history` returns them.
    Ndjson,
}

/// The file format of `GET /api/v1/history/export`, beside the list's filters.
#[derive(Clone, Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HistoryExportQuery {
    /// `csv` (the default) or `ndjson`.
    #[param(inline)]
    pub format: Option<HistoryExportFormat>,
}

/// `400` for a format other than the two, with a stable code like every REST refusal.
#[must_use]
pub fn history_export_format_invalid() -> ApiError {
    ApiError::bad_request(
        "history.export_format_invalid",
        "The export format must be csv or ndjson",
    )
}

/// The requested format, read from the query string beside the filters; an extractor of its own
/// so a word that does not parse is answered with a code instead of axum's plain-text `400`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExportFormat(pub HistoryExportFormat);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ExportFormat {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<HistoryExportQuery>::try_from_uri(&parts.uri)
            .map(|axum::extract::Query(query)| Self(query.format.unwrap_or_default()))
            .map_err(|_| history_export_format_invalid())
    }
}

/// The history the filters match as a file, newest first, at most [`MAX_HISTORY_EXPORT`]
/// entries.
#[utoipa::path(
    get,
    path = "/api/v1/history/export",
    tag = "history",
    params(HistoryExportQuery, HistoryFilterQuery),
    responses(
        (status = 200, description = "The filtered history as CSV (`format=csv`) or newline-delimited JSON (`format=ndjson`)", content((String = "text/csv"), (String = "application/x-ndjson")), headers(("x-total-count" = u64, description = "How many entries the filters match; more than the file holds when it was cut"))),
        (status = 400, description = "history.filter_invalid or history.export_format_invalid"),
    )
)]
pub async fn export_download_history(
    State(state): State<AppState>,
    ExportFormat(format): ExportFormat,
    HistoryFilter(filter): HistoryFilter,
) -> Result<Response, ApiError> {
    let query = rd_db::HistoryQuery {
        search: filter.q,
        outcome: filter.outcome,
        kind: filter.kind,
        finished_from: filter.from,
        finished_to: filter.to,
        compat_visible_only: false,
        offset: 0,
        limit: Some(MAX_HISTORY_EXPORT),
    };
    let result = state.database.list_download_history(&query).await?;
    let (body, content_type, extension) = match format {
        HistoryExportFormat::Csv => (
            history_csv(&result.entries),
            "text/csv; charset=utf-8",
            "csv",
        ),
        HistoryExportFormat::Ndjson => (
            history_ndjson(&result.entries)?,
            "application/x-ndjson",
            "ndjson",
        ),
    };
    let name = format!(
        "rdownloader-history-{}.{extension}",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .map_err(anyhow::Error::new)?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CONTENT_DISPOSITION, disposition),
            (
                header::HeaderName::from_static("x-total-count"),
                HeaderValue::from(result.total),
            ),
        ],
        Body::from(body),
    )
        .into_response())
}

fn history_ndjson(entries: &[HistoryEntry]) -> Result<String, ApiError> {
    let mut body = String::new();
    for entry in entries {
        body.push_str(&serde_json::to_string(entry).map_err(anyhow::Error::new)?);
        body.push('\n');
    }
    Ok(body)
}

/// The columns of the CSV file, in order.
const CSV_COLUMNS: [&str; 12] = [
    "id",
    "name",
    "outcome",
    "kind",
    "category",
    "destination",
    "total_bytes",
    "file_count",
    "created_at",
    "finished_at",
    "error_code",
    "sources",
];

/// The entries as RFC 4180 CSV behind a byte order mark, so a spreadsheet reads the names'
/// accents as UTF-8. The sources share one cell, separated by spaces.
#[must_use]
pub fn history_csv(entries: &[HistoryEntry]) -> String {
    let mut body = String::from("\u{feff}");
    push_row(
        &mut body,
        CSV_COLUMNS.iter().map(|column| (*column).to_owned()),
    );
    for entry in entries {
        push_row(
            &mut body,
            [
                entry.id.to_string(),
                text_cell(&entry.name),
                word(&entry.outcome),
                word(&entry.kind),
                text_cell(entry.category.as_deref().unwrap_or_default()),
                text_cell(&entry.destination),
                entry.total_bytes.get().to_string(),
                entry.file_count.to_string(),
                entry.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                entry.finished_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                text_cell(entry.error_code.as_deref().unwrap_or_default()),
                text_cell(&entry.sources.join(" ")),
            ],
        );
    }
    body
}

fn push_row(body: &mut String, cells: impl IntoIterator<Item = String>) {
    let row = cells
        .into_iter()
        .map(|cell| quoted(&cell))
        .collect::<Vec<_>>()
        .join(",");
    body.push_str(&row);
    body.push_str("\r\n");
}

/// A cell in quotes when it holds a separator, a quote or a line break, its quotes doubled.
fn quoted(cell: &str) -> String {
    if cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_owned()
    }
}

/// Text that came from outside — a release name, an address — never starts a formula: a
/// spreadsheet would run `=HYPERLINK(...)` in a file name as soon as the file is opened, so a
/// leading `=`, `+`, `-`, `@`, tab or carriage return gets an apostrophe in front (OWASP's advice
/// for CSV injection).
fn text_cell(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

/// An enum's wire word (`completed`, `usenet`), as the JSON list spells it.
fn word<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rd_core::{ByteCount, DownloadKind, HistoryEntry, HistoryOutcome, PackageId};

    use super::{history_csv, history_ndjson};

    fn entry(name: &str) -> HistoryEntry {
        HistoryEntry {
            id: 7,
            package_id: PackageId::new(),
            name: name.to_owned(),
            kind: DownloadKind::Usenet,
            category: Some("Films".to_owned()),
            destination: "/data/Films".to_owned(),
            total_bytes: ByteCount::new(1_048_576).expect("bytes"),
            file_count: 3,
            sources: vec![
                "https://example.invalid/a".to_owned(),
                "https://example.invalid/b".to_owned(),
            ],
            outcome: HistoryOutcome::Failed,
            error_code: Some("usenet.job_hopeless".to_owned()),
            error_params: rd_core::MessageParams::default(),
            created_at: Utc
                .with_ymd_and_hms(2026, 10, 1, 8, 0, 0)
                .single()
                .expect("time"),
            finished_at: Utc
                .with_ymd_and_hms(2026, 10, 1, 9, 30, 0)
                .single()
                .expect("time"),
        }
    }

    #[test]
    fn the_csv_has_a_header_row_and_one_line_per_entry() {
        let csv = history_csv(&[entry("Plain")]);
        let mut lines = csv.trim_start_matches('\u{feff}').split("\r\n");
        assert_eq!(
            lines.next(),
            Some(
                "id,name,outcome,kind,category,destination,total_bytes,file_count,created_at,finished_at,error_code,sources"
            )
        );
        assert_eq!(
            lines.next(),
            Some(
                "7,Plain,failed,usenet,Films,/data/Films,1048576,3,2026-10-01T08:00:00Z,2026-10-01T09:30:00Z,usenet.job_hopeless,https://example.invalid/a https://example.invalid/b"
            )
        );
        assert_eq!(lines.next(), Some(""));
        assert!(csv.starts_with('\u{feff}'), "the byte order mark leads");
    }

    #[test]
    fn a_separator_or_quote_is_quoted_and_a_formula_is_defused() {
        let csv = history_csv(&[entry("Say \"hi\", then go"), entry("=HYPERLINK(\"x\")")]);
        assert!(csv.contains(",\"Say \"\"hi\"\", then go\","), "{csv}");
        assert!(csv.contains(",\"'=HYPERLINK(\"\"x\"\")\","), "{csv}");
    }

    #[test]
    fn the_ndjson_is_one_entry_per_line_as_the_list_returns_it() {
        let ndjson = history_ndjson(&[entry("One"), entry("Two")]).expect("ndjson");
        let lines = ndjson.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["name"], "One");
        assert_eq!(first["outcome"], "failed");
    }
}

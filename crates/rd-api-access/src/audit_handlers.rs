//! Reading and exporting the audit log over REST (RD-110-03).
//!
//! Two operations, both reads. There is deliberately no write, no edit and no delete: the
//! records come from the actions themselves (`crate::audit::record`), the table refuses an
//! update outright (migration `0079`), and the only thing that removes a record is the
//! retention sweep. An endpoint that could clear the log would make the log worthless.
//!
//! Both cost `api:admin`. An audit log names who acted, from which address, on what — it is
//! the most concentrated description of an installation's activity the service holds, and a
//! read token pasted into a status page must not reach it. That is the same reasoning the
//! diagnostic log carries, one step sharper.

use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, SecondsFormat, Utc};
use rd_core::{AuditAction, AuditActorKind, AuditOutcome, AuditRetentionSettings};
use rd_db::AuditQuery;

use crate::{
    AppState,
    audit_dto::{
        AuditQueryParams, AuditRecordResponse, AuditRecordsResponse, AuditRetentionResponse,
        DEFAULT_AUDIT_PAGE, MAX_AUDIT_EXPORT, MAX_AUDIT_PAGE,
    },
    error::ApiError,
};

fn invalid(field: &str, message: &str) -> ApiError {
    ApiError::bad_request("audit.invalid_query", message).with_param("field", field)
}

fn parse_moment(value: Option<&str>, name: &str) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(text) = value.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(text)
        .map(|moment| Some(moment.with_timezone(&Utc)))
        .map_err(|_| invalid(name, "An audit filter value could not be read"))
}

fn trimmed(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Turns the query string into the store's query, refusing what it cannot mean.
///
/// `ceiling` differs between the page read and the export, and nothing else does: the same
/// filters, read the same way, so an export is exactly what the viewer is showing.
fn to_query(params: &AuditQueryParams, ceiling: u32) -> Result<AuditQuery, ApiError> {
    let action = match trimmed(params.action.as_ref()) {
        Some(word) => Some(
            AuditAction::parse(&word).ok_or_else(|| invalid("action", "Unknown audit action"))?,
        ),
        None => None,
    };
    let outcome = match trimmed(params.outcome.as_ref()) {
        Some(word) => {
            Some(AuditOutcome::parse(&word).ok_or_else(|| invalid("outcome", "Unknown outcome"))?)
        }
        None => None,
    };
    let actor_kind = match trimmed(params.actor_kind.as_ref()) {
        Some(word) => Some(
            AuditActorKind::parse(&word).ok_or_else(|| invalid("actor_kind", "Unknown actor"))?,
        ),
        None => None,
    };
    let limit = params.limit.unwrap_or(DEFAULT_AUDIT_PAGE);
    if limit == 0 || limit > ceiling {
        return Err(invalid(
            "limit",
            &format!("The page size must be between 1 and {ceiling}"),
        ));
    }
    Ok(AuditQuery {
        action,
        outcome,
        actor_kind,
        actor_id: trimmed(params.actor_id.as_ref()),
        target_kind: trimmed(params.target_kind.as_ref()),
        target_id: trimmed(params.target_id.as_ref()),
        trace_id: trimmed(params.trace_id.as_ref()),
        since: parse_moment(params.since.as_deref(), "since")?,
        until: parse_moment(params.until.as_deref(), "until")?,
        before_id: params.before_id,
        limit,
    })
}

fn to_response(record: rd_db::AuditRecord) -> AuditRecordResponse {
    AuditRecordResponse {
        id: record.id,
        recorded_at: record
            .recorded_at
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        action: record.action,
        outcome: record.outcome,
        actor_kind: record.actor_kind,
        actor_id: record.actor_id,
        actor_label: record.actor_label,
        client_address: record.client_address,
        target_kind: record.target_kind,
        target_id: record.target_id,
        target_name: record.target_name,
        trace_id: record.trace_id,
        details: record.details,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/audit/records",
    tag = "audit",
    params(AuditQueryParams),
    responses(
        (status = 200, body = AuditRecordsResponse),
        (status = 400, description = "audit.invalid_query")
    )
)]
pub async fn list_audit_records(
    State(state): State<AppState>,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<AuditRecordsResponse>, ApiError> {
    let query = to_query(&params, MAX_AUDIT_PAGE)?;
    let records = state.database.query_audit_records(&query).await?;
    let total = state.database.count_audit_records().await?;
    let retention: AuditRetentionSettings = state.database.service_settings_or_default().await?;
    let full_page = records.len() as u32 >= query.limit;
    Ok(Json(AuditRecordsResponse {
        records: records.into_iter().map(to_response).collect(),
        full_page,
        total,
        retention: AuditRetentionResponse {
            records: retention.audit_retention_records,
            days: retention.audit_retention_days,
        },
        actions: AuditAction::ALL.to_vec(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/audit/export",
    tag = "audit",
    params(AuditQueryParams),
    responses(
        (status = 200, description = "The filtered records as newline-delimited JSON", body = String, content_type = "application/x-ndjson"),
        (status = 400, description = "audit.invalid_query")
    )
)]
pub async fn export_audit_records(
    State(state): State<AppState>,
    Query(params): Query<AuditQueryParams>,
) -> Result<Response, ApiError> {
    // Newline-delimited JSON rather than CSV: an audit record carries a nested `details`
    // object and optional columns, and flattening that into a spreadsheet either loses the
    // details or invents a column per key. NDJSON streams into every log pipeline as it is,
    // and one record per line is still readable in a text editor.
    let mut query = to_query(&params, MAX_AUDIT_EXPORT)?;
    if params.limit.is_none() {
        query.limit = MAX_AUDIT_EXPORT;
    }
    let records = state.database.query_audit_records(&query).await?;
    let mut body = String::new();
    for record in records {
        let line = serde_json::to_string(&to_response(record)).map_err(anyhow::Error::new)?;
        body.push_str(&line);
        body.push('\n');
    }
    let name = format!(
        "rdownloader-audit-{}.ndjson",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .map_err(anyhow::Error::new)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(body),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::{AuditQueryParams, MAX_AUDIT_EXPORT, MAX_AUDIT_PAGE, to_query};

    fn params() -> AuditQueryParams {
        AuditQueryParams::default()
    }

    #[test]
    fn an_unknown_filter_word_is_refused_rather_than_ignored() {
        for (field, mut given) in [
            ("action", params()),
            ("outcome", params()),
            ("actor_kind", params()),
        ] {
            match field {
                "action" => given.action = Some("deleted_everything".to_owned()),
                "outcome" => given.outcome = Some("maybe".to_owned()),
                _ => given.actor_kind = Some("root".to_owned()),
            }
            let error = to_query(&given, MAX_AUDIT_PAGE).expect_err("refused");
            assert_eq!(error.code(), "audit.invalid_query", "for {field}");
        }
    }

    #[test]
    fn the_page_ceiling_differs_between_the_viewer_and_the_export() {
        let mut given = params();
        given.limit = Some(MAX_AUDIT_PAGE + 1);
        assert!(to_query(&given, MAX_AUDIT_PAGE).is_err());
        assert!(to_query(&given, MAX_AUDIT_EXPORT).is_ok());
        given.limit = Some(MAX_AUDIT_EXPORT + 1);
        assert!(to_query(&given, MAX_AUDIT_EXPORT).is_err());
        given.limit = Some(0);
        assert!(to_query(&given, MAX_AUDIT_EXPORT).is_err());
    }

    #[test]
    fn a_blank_filter_is_no_filter_and_a_bad_moment_is_refused() {
        let mut given = params();
        given.actor_id = Some("   ".to_owned());
        given.trace_id = Some(String::new());
        let query = to_query(&given, MAX_AUDIT_PAGE).expect("accepted");
        assert!(query.actor_id.is_none() && query.trace_id.is_none());
        given.since = Some("yesterday".to_owned());
        assert!(to_query(&given, MAX_AUDIT_PAGE).is_err());
    }
}

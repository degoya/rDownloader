//! Export and import of the person's site rules: the exchange file (RD-1230-03).
//!
//! What one installation exports another imports without a key and without rework: every rule
//! with the switch it had at the exporter. The import runs in two requests on purpose. The
//! preview reads the file and answers, per rule, what an import would do -- `new`, `replaces`,
//! `same` or `refused` -- and stores nothing; the import then writes the new rules and replaces
//! a stored one only when the request names its id in `replace`, which the dialog asks first.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::Query;

use super::*;

#[utoipa::path(
    get,
    path = "/api/v1/site-rules/export",
    tag = "configuration",
    params(SiteRuleExportQuery),
    responses((status = 200, body = SiteRuleDocument))
)]
pub async fn export_site_rules(
    State(state): State<AppState>,
    Query(query): Query<SiteRuleExportQuery>,
) -> Result<Json<SiteRuleDocument>, ApiError> {
    let wanted: Option<BTreeSet<String>> = query.ids.as_deref().map(|ids| {
        ids.split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect()
    });
    let rules = state
        .database
        .list_site_rules()
        .await?
        .into_iter()
        .filter(|stored| wanted.as_ref().is_none_or(|ids| ids.contains(&stored.id)))
        .map(|stored| SiteRuleDocumentEntry {
            enabled: stored.enabled,
            rule: stored.rule,
        })
        .collect();
    Ok(Json(SiteRuleDocument {
        format_version: rd_siterules::EXCHANGE_VERSION,
        rules,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-rules/import/preview",
    tag = "configuration",
    request_body(content = SiteRuleDocument, content_type = "application/json"),
    responses((status = 200, body = SiteRuleImportPreviewResponse))
)]
pub async fn preview_site_rule_import(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<Json<SiteRuleImportPreviewResponse>, ApiError> {
    rd_api_core::input_checks::require_media_type(&headers, "application/json")?;
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(malformed)?;
    let document = read_document(value)?;
    let stored = stored_by_id(&state).await?;
    let rules = plan(&document, &stored)
        .into_iter()
        .map(|planned| {
            let status = planned.verdict.preview_word();
            planned.response(status)
        })
        .collect();
    Ok(Json(SiteRuleImportPreviewResponse { rules }))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-rules/import",
    tag = "configuration",
    request_body(content = SiteRuleImportRequest, content_type = "application/json"),
    responses((status = 200, body = ImportSiteRulesResponse))
)]
pub async fn import_site_rules(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<Json<ImportSiteRulesResponse>, ApiError> {
    rd_api_core::input_checks::require_media_type(&headers, "application/json")?;
    let mut request: serde_json::Value = serde_json::from_slice(&body).map_err(malformed)?;
    let document = read_document(
        request
            .get_mut("document")
            .map(serde_json::Value::take)
            .ok_or_else(|| {
                ApiError::bad_request("site_rules.malformed", "The import carries no document")
            })?,
    )?;
    let replace: BTreeSet<String> = match request.get_mut("replace").map(serde_json::Value::take) {
        None | Some(serde_json::Value::Null) => BTreeSet::new(),
        Some(ids) => serde_json::from_value(ids).map_err(malformed)?,
    };
    let stored = stored_by_id(&state).await?;
    let mut results = Vec::with_capacity(document.rules.len());
    let (mut written, mut replaced) = (0usize, 0usize);
    for planned in plan(&document, &stored) {
        let status = match (&planned.verdict, &planned.rule) {
            (Verdict::New, Some(rule)) => {
                persist(&state, rule, planned.enabled, SiteRuleOriginKind::Import).await?;
                written += 1;
                "stored"
            }
            (Verdict::Replaces, Some(rule)) if replace.contains(&rule.id) => {
                persist(&state, rule, planned.enabled, SiteRuleOriginKind::Import).await?;
                replaced += 1;
                "replaced"
            }
            (Verdict::Replaces, _) => "kept",
            (Verdict::Same, _) => "same",
            (Verdict::Refused(_), _) | (Verdict::New, None) => "refused",
        };
        results.push(planned.response(status));
    }
    if written + replaced > 0 {
        reload(&state).await;
    }
    Ok(Json(ImportSiteRulesResponse {
        rules: results,
        stored: written,
        replaced,
    }))
}

fn malformed(error: serde_json::Error) -> ApiError {
    ApiError::bad_request("site_rules.malformed", error.to_string())
}

/// Reads the exchange file, its layout first: a file of another `format_version` -- the bodies
/// alone of 1.22 and before, or a later layout -- is refused whole with
/// `site_rules.format_version_unsupported`, before its entries are read at all.
pub(super) fn read_document(value: serde_json::Value) -> Result<SiteRuleDocument, ApiError> {
    let version = value
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ApiError::bad_request(
                "site_rules.malformed",
                "This is not a site-rule export: format_version is missing",
            )
        })?;
    if version != u64::from(rd_siterules::EXCHANGE_VERSION) {
        return Err(ApiError::bad_request(
            "site_rules.format_version_unsupported",
            "This build does not read that rule format",
        )
        .with_param("format_version", version));
    }
    serde_json::from_value(value).map_err(malformed)
}

/// Every stored rule by id, as the comparison needs it.
async fn stored_by_id(state: &AppState) -> Result<BTreeMap<String, rd_db::UserSiteRule>, ApiError> {
    Ok(state
        .database
        .list_site_rules()
        .await?
        .into_iter()
        .map(|stored| (stored.id.clone(), stored))
        .collect())
}

/// What an import does with one rule of the file.
enum Verdict {
    /// No stored rule carries the id.
    New,
    /// A stored rule carries the id and differs in its body or its switch.
    Replaces,
    /// A stored rule carries the id with the same body and the same switch.
    Same,
    /// The body does not read or validate, or its id came earlier in the same file.
    Refused(String),
}

impl Verdict {
    fn preview_word(&self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Replaces => "replaces",
            Self::Same => "same",
            Self::Refused(_) => "refused",
        }
    }
}

/// One rule of the file, read and compared.
struct Planned {
    id: String,
    name: String,
    hosts: Vec<String>,
    enabled: bool,
    verdict: Verdict,
    /// The parsed rule, absent exactly when the verdict is a refusal.
    rule: Option<Rule>,
}

impl Planned {
    fn response(self, status: &str) -> ImportedSiteRuleResponse {
        let code = match self.verdict {
            Verdict::Refused(code) => Some(code),
            _ => None,
        };
        ImportedSiteRuleResponse {
            id: self.id,
            name: self.name,
            hosts: self.hosts,
            enabled: self.enabled,
            status: status.to_owned(),
            code,
        }
    }
}

/// Reads every rule of the file and compares it with what is stored, writing nothing.
fn plan(
    document: &SiteRuleDocument,
    stored: &BTreeMap<String, rd_db::UserSiteRule>,
) -> Vec<Planned> {
    let mut seen = BTreeSet::new();
    document
        .rules
        .iter()
        .map(|entry| {
            let text = |key: &str| {
                entry
                    .rule
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let rule = match parse_rule(&entry.rule) {
                Ok(rule) => rule,
                Err(error) => {
                    return Planned {
                        id: text("id"),
                        name: text("name"),
                        hosts: Vec::new(),
                        enabled: entry.enabled,
                        verdict: Verdict::Refused(error.code().to_owned()),
                        rule: None,
                    };
                }
            };
            let verdict = if !seen.insert(rule.id.clone()) {
                Verdict::Refused("site_rules.duplicate_id".to_owned())
            } else {
                match stored.get(&rule.id) {
                    None => Verdict::New,
                    Some(existing)
                        if existing.enabled == entry.enabled
                            && serde_json::to_value(&rule).ok().as_ref()
                                == Some(&existing.rule) =>
                    {
                        Verdict::Same
                    }
                    Some(_) => Verdict::Replaces,
                }
            };
            let refused = matches!(verdict, Verdict::Refused(_));
            Planned {
                id: rule.id.clone(),
                name: rule.name.clone(),
                hosts: rule.matches.hosts.clone(),
                enabled: entry.enabled,
                verdict,
                rule: (!refused).then_some(rule),
            }
        })
        .collect()
}

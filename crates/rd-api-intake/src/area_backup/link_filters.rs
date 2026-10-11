//! Export and import of the LinkFilter rules (RD-1240-09).

use super::*;

/// One LinkFilter rule, with its category named rather than referenced. The order of the list
/// is the evaluation order; an import appends in it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAreaLinkFilter {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub name_pattern: Option<String>,
    #[serde(default)]
    pub name_syntax: rd_core::LinkFilterNameSyntax,
    #[serde(default)]
    pub size_min: Option<u64>,
    #[serde(default)]
    pub size_max: Option<u64>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub hoster: Option<String>,
    #[serde(default)]
    pub source: Option<rd_core::IngressSource>,
    pub action: rd_core::LinkFilterAction,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub category_name: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/link-filters/export", tag = "collector", responses((status = 200, body = AreaBundle)))]
pub async fn export_link_filters(
    State(state): State<AppState>,
) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let entries = state
        .database
        .list_link_filter_rules()
        .await?
        .into_iter()
        .map(|rule| BundleAreaLinkFilter {
            category_name: rule.category_id.and_then(|id| {
                categories
                    .iter()
                    .find(|category| category.id == id)
                    .map(|category| category.name.clone())
            }),
            name: rule.name,
            enabled: rule.enabled,
            name_pattern: rule.name_pattern,
            name_syntax: rule.name_syntax,
            size_min: rule.size_min,
            size_max: rule.size_max,
            extensions: rule.extensions,
            hoster: rule.hoster,
            source: rule.source,
            action: rule.action,
            package_name: rule.package_name,
        })
        .collect();
    Ok(Json(AreaBundle {
        link_filters: Some(entries),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/link-filters/import", tag = "collector", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_link_filters(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    let entries = bundle
        .link_filters
        .ok_or_else(|| section_missing("link_filters"))?;
    crate::error_codes::validate_bundle_section(entries.len())?;
    let categories = state.database.list_categories().await?;
    let mut existing: HashSet<String> = state
        .database
        .list_link_filter_rules()
        .await?
        .into_iter()
        .map(|rule| rule.name)
        .collect();
    let mut summary = ImportAreaSummary::default();
    for entry in entries {
        if existing.contains(&entry.name) {
            summary.skipped += 1;
            continue;
        }
        // A rule whose category this instance does not have would file links somewhere else
        // than the rule that was exported; skipped rather than imported half.
        let category_id = match &entry.category_name {
            None => None,
            Some(name) => match categories.iter().find(|category| &category.name == name) {
                Some(category) => Some(category.id),
                None => {
                    summary.skipped += 1;
                    continue;
                }
            },
        };
        let request = crate::link_filter_input::LinkFilterRuleRequest {
            name: entry.name.clone(),
            enabled: entry.enabled,
            name_pattern: entry.name_pattern,
            name_syntax: entry.name_syntax,
            size_min: entry.size_min,
            size_max: entry.size_max,
            extensions: entry.extensions,
            hoster: entry.hoster,
            source: entry.source,
            action: entry.action,
            package_name: entry.package_name,
            category_id,
        };
        let Ok(input) = crate::link_filter_input::validated_link_filter_rule(&state, request).await
        else {
            summary.skipped += 1;
            continue;
        };
        state.database.create_link_filter_rule(input).await?;
        existing.insert(entry.name);
        summary.created += 1;
    }
    Ok(Json(summary))
}

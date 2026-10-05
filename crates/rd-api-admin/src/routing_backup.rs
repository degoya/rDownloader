//! Export/import of the routing configuration (categories + category rules).
//!
//! Unlike the full settings backup this is a merge: entries are matched by name, existing
//! ones are kept untouched and conflicting or unresolvable entries are counted as skipped.
//! The import loops over the regular facade creates (which emit `CategoryChanged` events
//! themselves), so it is not atomic — but a re-import of the same bundle is idempotent.

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    ApiError, AppState,
    dto::{CreateCategoryRequest, CreateCategoryRuleRequest},
};

const BUNDLE_FORMAT: &str = "rdownloader-routing-bundle";
const BUNDLE_VERSION: u32 = 1;

/// One category in a routing bundle. Carries the storage root by name instead of id so the
/// import can attach it to the target instance's root of the same name.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleRoutingCategory {
    pub name: String,
    pub color: String,
    pub storage_root_name: String,
    pub relative_path: String,
    pub is_default: bool,
    #[serde(default)]
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    #[serde(default)]
    pub unpack_to_subfolder: Option<bool>,
    #[serde(default)]
    pub direct_unpack: Option<bool>,
    #[serde(default)]
    pub malware_scan: Option<bool>,
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    #[serde(default)]
    pub delete_par2: Option<bool>,
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    #[serde(default)]
    pub upload_remote: Option<String>,
}

/// One category rule in a routing bundle; the target category is referenced by name.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleRoutingRule {
    pub name: String,
    pub priority: i32,
    #[serde(default)]
    pub source: Option<rd_core::IngressSource>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub extension: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub name_regex: Option<String>,
    pub category_name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RoutingBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub app_version: String,
    #[serde(default)]
    pub categories: Vec<BundleRoutingCategory>,
    #[serde(default)]
    pub rules: Vec<BundleRoutingRule>,
}

#[derive(Debug, Default, Serialize, ToSchema)]
pub struct ImportRoutingSummary {
    pub categories_created: u32,
    pub categories_skipped: u32,
    pub rules_created: u32,
    pub rules_skipped: u32,
}

/// Which part of the routing configuration an export carries.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingExportPart {
    /// Categories and their rules together — what the export always was.
    #[default]
    All,
    /// The categories alone.
    Categories,
    /// The rules alone; an import matches their categories by name on the target.
    Rules,
}

/// `GET /api/v1/routing/export?part=…`; without `part` the export carries everything.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct RoutingExportParams {
    pub part: Option<RoutingExportPart>,
}

#[utoipa::path(
    get,
    path = "/api/v1/routing/export",
    tag = "configuration",
    params(RoutingExportParams),
    responses((status = 200, body = RoutingBundle))
)]
pub async fn export_routing(
    State(state): State<AppState>,
    Query(params): Query<RoutingExportParams>,
) -> Result<Json<RoutingBundle>, ApiError> {
    let part = params.part.unwrap_or_default();
    let (roots, categories, rules) = tokio::try_join!(
        async { Ok::<_, ApiError>(state.database.list_storage_roots().await?) },
        async { Ok::<_, ApiError>(state.database.list_categories().await?) },
        async { Ok::<_, ApiError>(state.database.list_category_rules().await?) },
    )?;
    let root_name = |id: rd_core::StorageRootId| {
        roots
            .iter()
            .find(|root| root.id == id)
            .map(|root| root.name.clone())
    };
    let category_name = |id: rd_core::CategoryId| {
        categories
            .iter()
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
    };
    let bundled_categories = categories
        .iter()
        .filter_map(|category| {
            Some(BundleRoutingCategory {
                name: category.name.clone(),
                color: category.color.clone(),
                storage_root_name: root_name(category.storage_root_id)?,
                relative_path: category.relative_path.clone(),
                is_default: category.is_default,
                postprocess_level: category.postprocess_level,
                script: category.script.clone(),
                cleanup_extensions: category.cleanup_extensions.clone(),
                recursive_unpack: category.recursive_unpack,
                unpack_to_subfolder: category.unpack_to_subfolder,
                direct_unpack: category.direct_unpack,
                malware_scan: category.malware_scan,
                sfv_verify: category.sfv_verify,
                safe_postproc: category.safe_postproc,
                delete_par2: category.delete_par2,
                upload_enabled: category.upload_enabled,
                upload_remote: category.upload_remote.clone(),
            })
        })
        .collect();
    let bundled_rules = rules
        .iter()
        .filter_map(|rule| {
            Some(BundleRoutingRule {
                name: rule.name.clone(),
                priority: rule.priority,
                source: rule.source,
                domain: rule.domain.clone(),
                protocol: rule.protocol.clone(),
                extension: rule.extension.clone(),
                mime_type: rule.mime_type.clone(),
                name_regex: rule.name_regex.clone(),
                category_name: category_name(rule.category_id)?,
                enabled: rule.enabled,
            })
        })
        .collect();
    Ok(Json(RoutingBundle {
        format: BUNDLE_FORMAT.to_owned(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        categories: if part == RoutingExportPart::Rules {
            Vec::new()
        } else {
            bundled_categories
        },
        rules: if part == RoutingExportPart::Categories {
            Vec::new()
        } else {
            bundled_rules
        },
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/routing/import",
    tag = "configuration",
    request_body = RoutingBundle,
    responses((status = 200, body = ImportRoutingSummary), (status = 400, body = crate::error::ErrorBody))
)]
pub async fn import_routing(
    State(state): State<AppState>,
    Json(bundle): Json<RoutingBundle>,
) -> Result<Json<ImportRoutingSummary>, ApiError> {
    validate_header(&bundle)?;
    // Checked before the first write, as for an area bundle: an oversized bundle is refused
    // rather than half-applied.
    crate::error_codes::validate_bundle_section(bundle.categories.len())?;
    crate::error_codes::validate_bundle_section(bundle.rules.len())?;
    let roots = state.database.list_storage_roots().await?;
    let categories = state.database.list_categories().await?;
    let existing_rules = state.database.list_category_rules().await?;
    let mut summary = ImportRoutingSummary::default();
    let mut has_default = categories.iter().any(|category| category.is_default);
    // Name to id of the target's categories, kept current as the import creates more. The
    // first category of a name wins, as a lookup in list order did.
    let mut category_ids: HashMap<String, rd_core::CategoryId> = HashMap::new();
    for category in categories {
        category_ids.entry(category.name).or_insert(category.id);
    }
    // Kept current too, so a bundle naming one rule twice creates it once.
    let mut rule_names: HashSet<String> =
        existing_rules.into_iter().map(|rule| rule.name).collect();
    for entry in bundle.categories {
        if category_ids.contains_key(&entry.name) {
            summary.categories_skipped += 1;
            continue;
        }
        let Some(root) = roots
            .iter()
            .find(|root| root.name == entry.storage_root_name)
        else {
            summary.categories_skipped += 1;
            continue;
        };
        let request = CreateCategoryRequest {
            name: entry.name,
            color: entry.color,
            storage_root_id: root.id,
            relative_path: entry.relative_path,
            // Never displace the target's default category; only the first imported default
            // may claim the slot when the target has none.
            is_default: entry.is_default && !has_default,
            postprocess_level: entry.postprocess_level,
            script: entry.script,
            cleanup_extensions: entry.cleanup_extensions,
            recursive_unpack: entry.recursive_unpack,
            unpack_to_subfolder: entry.unpack_to_subfolder,
            direct_unpack: entry.direct_unpack,
            malware_scan: entry.malware_scan,
            sfv_verify: entry.sfv_verify,
            safe_postproc: entry.safe_postproc,
            delete_par2: entry.delete_par2,
            upload_enabled: entry.upload_enabled,
            upload_remote: entry.upload_remote,
        };
        let Ok(input) = crate::config_handlers::validated_category(&state, request).await else {
            summary.categories_skipped += 1;
            continue;
        };
        has_default = has_default || input.is_default;
        let created = state.database.create_category(input).await?;
        category_ids.entry(created.name).or_insert(created.id);
        summary.categories_created += 1;
    }
    for entry in bundle.rules {
        if rule_names.contains(&entry.name) {
            summary.rules_skipped += 1;
            continue;
        }
        // Resolve against current + just-created categories, so a rule may attach to a
        // pre-existing category of the same name even when its bundle category was skipped.
        let Some(&category_id) = category_ids.get(&entry.category_name) else {
            summary.rules_skipped += 1;
            continue;
        };
        let name = entry.name.clone();
        let request = CreateCategoryRuleRequest {
            name: entry.name,
            priority: entry.priority,
            source: entry.source,
            domain: entry.domain,
            protocol: entry.protocol,
            extension: entry.extension,
            mime_type: entry.mime_type,
            name_regex: entry.name_regex,
            category_id,
            enabled: entry.enabled,
        };
        let Ok(input) = crate::config_handlers::validated_category_rule(&state, request).await
        else {
            summary.rules_skipped += 1;
            continue;
        };
        state.database.create_category_rule(input).await?;
        rule_names.insert(name);
        summary.rules_created += 1;
    }
    Ok(Json(summary))
}

/// What a routing bundle says about itself; only the current version is read. The refused
/// version travels as a parameter rather than inside the text (audit 1.9.1, API-11).
const BUNDLE_HEADER: rd_api_core::input_checks::BundleHeader =
    rd_api_core::input_checks::BundleHeader {
        format: BUNDLE_FORMAT,
        version: BUNDLE_VERSION,
        reads_older: false,
        format_code: "routing.backup_invalid",
        format_message: "The selected file is not an rDownloader routing bundle",
        version_code: "routing.backup_version_unsupported",
        version_message: "This routing bundle version is not supported",
    };

fn validate_header(bundle: &RoutingBundle) -> Result<(), ApiError> {
    BUNDLE_HEADER.check(&bundle.format, bundle.version)
}

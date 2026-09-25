//! Export/import of the routing configuration (categories + category rules).
//!
//! Unlike the full settings backup this is a merge: entries are matched by name, existing
//! ones are kept untouched and conflicting or unresolvable entries are counted as skipped.
//! The import loops over the regular facade creates (which emit `CategoryChanged` events
//! themselves), so it is not atomic — but a re-import of the same bundle is idempotent.

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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

#[utoipa::path(
    get,
    path = "/api/v1/routing/export",
    tag = "configuration",
    responses((status = 200, body = RoutingBundle))
)]
pub async fn export_routing(
    State(state): State<AppState>,
) -> Result<Json<RoutingBundle>, ApiError> {
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
        categories: bundled_categories,
        rules: bundled_rules,
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
    let roots = state.database.list_storage_roots().await?;
    let mut categories = state.database.list_categories().await?;
    let existing_rules = state.database.list_category_rules().await?;
    let mut summary = ImportRoutingSummary::default();
    let mut has_default = categories.iter().any(|category| category.is_default);
    for entry in bundle.categories {
        if categories
            .iter()
            .any(|category| category.name == entry.name)
        {
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
        categories.push(created);
        summary.categories_created += 1;
    }
    for entry in bundle.rules {
        if existing_rules.iter().any(|rule| rule.name == entry.name) {
            summary.rules_skipped += 1;
            continue;
        }
        // Resolve against current + just-created categories, so a rule may attach to a
        // pre-existing category of the same name even when its bundle category was skipped.
        let Some(category) = categories
            .iter()
            .find(|category| category.name == entry.category_name)
        else {
            summary.rules_skipped += 1;
            continue;
        };
        let request = CreateCategoryRuleRequest {
            name: entry.name,
            priority: entry.priority,
            source: entry.source,
            domain: entry.domain,
            protocol: entry.protocol,
            extension: entry.extension,
            mime_type: entry.mime_type,
            name_regex: entry.name_regex,
            category_id: category.id,
            enabled: entry.enabled,
        };
        let Ok(input) = crate::config_handlers::validated_category_rule(&state, request).await
        else {
            summary.rules_skipped += 1;
            continue;
        };
        state.database.create_category_rule(input).await?;
        summary.rules_created += 1;
    }
    Ok(Json(summary))
}

fn validate_header(bundle: &RoutingBundle) -> Result<(), ApiError> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(ApiError::bad_request(
            "routing.backup_invalid",
            "The selected file is not an rDownloader routing bundle",
        ));
    }
    if bundle.version != BUNDLE_VERSION {
        return Err(ApiError::bad_request(
            "routing.backup_version_unsupported",
            format!("Routing bundle version {} is not supported", bundle.version),
        ));
    }
    Ok(())
}

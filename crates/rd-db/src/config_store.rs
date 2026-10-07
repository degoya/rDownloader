use anyhow::Result;
use rd_core::{
    Category, CategoryId, EventEnvelope, EventKind, HotFolderExecutor, ImportMode, IngressSource,
    StorageRootConfig, StorageRootId,
};

#[path = "config_store_categories.rs"]
mod categories;
#[path = "config_store_hotfolders.rs"]
mod hotfolders;
#[path = "config_store_postprocess.rs"]
mod postprocess;
#[path = "config_store_rules.rs"]
mod rules;
#[path = "config_store_storage_roots.rs"]
mod storage_roots;

pub(crate) use categories::{create_category, delete_category, list_categories, update_category};
pub(crate) use hotfolders::{
    create_hotfolder, delete_hotfolder, list_hotfolders, update_hotfolder,
};
pub(crate) use postprocess::{
    package_name_regex_json, package_name_rules_json, set_category_seeding, sorting_json,
    update_category_postprocess,
};
pub(crate) use rules::{
    create_category_rule, delete_category_rule, list_category_rules, routing_config,
    update_category_rule,
};
pub(crate) use storage_roots::{
    create_storage_root, default_storage_root, delete_storage_root, list_storage_roots,
    update_storage_root,
};

#[derive(Clone, Debug)]
pub struct NewStorageRoot {
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// Free space kept on this root; `None` inherits the global threshold.
    pub minimum_free_bytes: Option<rd_core::ByteCount>,
}

#[derive(Clone, Debug)]
pub struct NewCategory {
    pub name: String,
    pub color: String,
    pub storage_root_id: StorageRootId,
    pub relative_path: String,
    pub is_default: bool,
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    pub script: Option<String>,
    pub cleanup_extensions: Option<Vec<String>>,
    pub recursive_unpack: Option<bool>,
    pub unpack_to_subfolder: Option<bool>,
    pub direct_unpack: Option<bool>,
    pub malware_scan: Option<bool>,
    pub sfv_verify: Option<bool>,
    pub safe_postproc: Option<bool>,
    pub delete_par2: Option<bool>,
    pub upload_enabled: Option<bool>,
    pub upload_remote: Option<String>,
    pub unwrap_package_folder: Option<bool>,
}

/// A category's post-processing overrides; `None` per field inherits the global setting.
///
/// These values are only ever read and written together, so they travel as one value
/// instead of as a row of positional parameters that are easy to transpose at a call site.
#[derive(Clone, Debug, Default)]
pub struct CategoryPostprocess {
    pub level: Option<rd_core::PostprocessLevel>,
    pub script: Option<String>,
    pub cleanup_extensions: Option<Vec<String>>,
    pub recursive_unpack: Option<bool>,
    pub unpack_to_subfolder: Option<bool>,
    pub direct_unpack: Option<bool>,
    pub malware_scan: Option<bool>,
    pub sfv_verify: Option<bool>,
    pub safe_postproc: Option<bool>,
    pub delete_par2: Option<bool>,
    /// Plugin steps for this category, by plugin id; `None` inherits, an empty list means
    /// "none here" and switches a globally enabled step off.
    pub plugin_steps: Option<Vec<String>>,
    pub upload_enabled: Option<bool>,
    pub upload_remote: Option<String>,
    /// Sort and rename templates (RD-1100-08); `None` = no sorting. Not an override: there is
    /// no global template to inherit.
    pub sorting: Option<rd_core::SortTemplates>,
    pub unwrap_package_folder: Option<bool>,
    /// Package-name rules override (RD-1140-05); `None`, or an override that sets nothing,
    /// inherits every global switch.
    pub package_name_rules: Option<rd_core::PackageNameRulesOverride>,
    /// Regex pairs (RD-1140-05); `None` inherits the global list, a list replaces it.
    pub package_name_regex: Option<Vec<rd_core::PackageNameRegex>>,
}

#[derive(Clone, Debug)]
pub struct NewCategoryRule {
    pub name: String,
    pub priority: i32,
    pub source: Option<IngressSource>,
    pub domain: Option<String>,
    pub protocol: Option<String>,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub name_regex: Option<String>,
    pub name_target: rd_core::CategoryRuleNameTarget,
    pub category_id: CategoryId,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct NewHotFolder {
    pub name: String,
    pub executor: HotFolderExecutor,
    pub path: String,
    pub recursive: bool,
    pub category_id: Option<CategoryId>,
    pub import_mode: ImportMode,
    pub processed_path: String,
    pub failed_path: String,
    pub enabled: bool,
}

/// A storage root's name and its folder are each unique (`storage_roots`).
const STORAGE_ROOT_TAKEN: &str = "a storage root with this name or folder already exists";
/// A category's name is unique (`categories.name`).
const CATEGORY_NAME_TAKEN: &str = "a category with this name already exists";
/// A watched folder's name is unique, and so is its folder per executor (`hotfolders`).
const HOTFOLDER_TAKEN: &str = "a watched folder with this name or folder already exists";

const CATEGORY_COLUMNS: &str = "id, name, color, storage_root_id, relative_path, is_default, postprocess_level, script, cleanup_extensions, recursive_unpack, unpack_to_subfolder, direct_unpack, malware_scan, sfv_verify, safe_postproc, delete_par2, upload_enabled, upload_remote, seeding_json, plugin_steps_json, sorting_json, package_name_rules_json, package_name_regex_json, unwrap_package_folder";

/// Repairs the default flag across a restored bundle.
///
/// An exported bundle predates the single-default invariant, and a hand-edited one can say
/// anything, so restore repairs instead of refusing: keep the alphabetically first root that
/// claims the default, or promote the alphabetically first root when none does.
pub(crate) fn normalize_storage_root_defaults(roots: &mut [StorageRootConfig]) {
    let winner = roots
        .iter()
        .filter(|root| root.is_default)
        .min_by(|left, right| left.name.cmp(&right.name))
        .or_else(|| {
            roots
                .iter()
                .min_by(|left, right| left.name.cmp(&right.name))
        })
        .map(|root| root.id);
    for root in roots.iter_mut() {
        root.is_default = Some(root.id) == winner;
    }
}

/// Repairs the default flag across the categories of a restored bundle.
///
/// The same rule as for storage roots, and for the same reason: a bundle exported before the
/// invariant existed, or edited by hand, can carry none or several, and the partial unique
/// index would reject the restore outright. Repair instead of refusing — a restore may not
/// leave routing without its fallback category.
pub(crate) fn normalize_category_defaults(categories: &mut [Category]) {
    let winner = categories
        .iter()
        .filter(|category| category.is_default)
        .min_by(|left, right| left.name.cmp(&right.name))
        .or_else(|| {
            categories
                .iter()
                .min_by(|left, right| left.name.cmp(&right.name))
        })
        .map(|category| category.id);
    for category in categories.iter_mut() {
        category.is_default = Some(category.id) == winner;
    }
}

fn config_event<T: serde::Serialize>(kind: EventKind, resource: &str, id: T) -> EventEnvelope {
    EventEnvelope::new(kind, serde_json::json!({ "resource": resource, "id": id }))
}

fn cleanup_json(value: Option<&Vec<String>>) -> Result<Option<String>> {
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use rd_core::{StorageRootConfig, StorageRootId};

    use super::normalize_storage_root_defaults;

    fn root(name: &str, is_default: bool) -> StorageRootConfig {
        StorageRootConfig {
            id: StorageRootId::new(),
            name: name.to_owned(),
            path: format!("/{}", name.to_lowercase()),
            is_default,
            minimum_free_bytes: None,
        }
    }

    fn defaults(roots: &[StorageRootConfig]) -> Vec<&str> {
        roots
            .iter()
            .filter(|root| root.is_default)
            .map(|root| root.name.as_str())
            .collect()
    }

    #[test]
    fn a_bundle_without_a_default_gets_the_alphabetically_first_one() {
        let mut roots = vec![root("Zulu", false), root("Alpha", false)];

        normalize_storage_root_defaults(&mut roots);

        assert_eq!(defaults(&roots), vec!["Alpha"]);
    }

    #[test]
    fn a_bundle_with_several_defaults_keeps_the_alphabetically_first() {
        let mut roots = vec![root("Zulu", true), root("Alpha", true), root("Mike", false)];

        normalize_storage_root_defaults(&mut roots);

        assert_eq!(defaults(&roots), vec!["Alpha"]);
    }

    #[test]
    fn a_bundle_with_exactly_one_default_is_left_alone() {
        let mut roots = vec![root("Zulu", true), root("Alpha", false)];

        normalize_storage_root_defaults(&mut roots);

        assert_eq!(
            defaults(&roots),
            vec!["Zulu"],
            "a valid bundle must not be rewritten to the alphabetical choice"
        );
    }

    #[test]
    fn an_empty_bundle_stays_empty() {
        let mut roots: Vec<StorageRootConfig> = Vec::new();

        normalize_storage_root_defaults(&mut roots);

        assert!(roots.is_empty());
    }
}

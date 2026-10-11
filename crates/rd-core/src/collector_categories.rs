//! Categories, storage roots, category rules and hotfolder configuration.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::IngressSource;
use crate::{
    CaptureAgentId, CategoryId, CategoryRuleId, HotFolderId, PostprocessLevel, StorageRootId,
};

/// User-defined destination category.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Category {
    pub id: CategoryId,
    pub name: String,
    pub color: String,
    pub storage_root_id: StorageRootId,
    pub relative_path: String,
    pub is_default: bool,
    /// Default post-processing level for packages in this category; `None` = global default.
    #[serde(default)]
    pub postprocess_level: Option<PostprocessLevel>,
    /// Default post-processing script for packages in this category.
    #[serde(default)]
    pub script: Option<String>,
    /// Extensions (without dot) removed after unpacking packages in this category;
    /// `None` = use the global cleanup list.
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    /// Whether packages in this category unpack nested archives recursively; `None` = global default.
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    /// Whether packages in this category unpack every archive set into a folder of its own;
    /// `None` = global default (RD-170-16).
    #[serde(default)]
    pub unpack_to_subfolder: Option<bool>,
    /// Whether packages in this category unpack a multi-volume RAR set while it downloads;
    /// `None` = global default (RD-1100-07).
    #[serde(default)]
    pub direct_unpack: Option<bool>,
    /// Whether packages in this category are scanned by ClamAV before they count as finished;
    /// `None` = global default (RD-190-14).
    #[serde(default)]
    pub malware_scan: Option<bool>,
    /// Whether packages in this category verify `.sfv` checksums; `None` = global default.
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    /// Whether a failed verification blocks unpacking for packages in this category;
    /// `None` = global default (RD-104-04).
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    /// Whether packages in this category discard the PAR2 recovery set after a successful
    /// unpack; `None` = global default.
    #[serde(default)]
    pub delete_par2: Option<bool>,
    /// Whether packages in this category upload to rclone; `None` = global default.
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    /// rclone target override in `remote:path` form; `None` = the global remote.
    #[serde(default)]
    pub upload_remote: Option<String>,
    /// Seeding override for torrents in this category; every unset field inherits from
    /// the global settings.
    #[serde(default)]
    pub seeding: Option<crate::SeedingPolicyOverride>,
    /// Post-processing plugin steps for packages in this category, by plugin id and in the
    /// order they run; `None` = the global list. An empty list means "none here", which is
    /// how a category switches a globally enabled step off.
    #[serde(default)]
    pub plugin_steps: Option<Vec<String>>,
    /// Sort and rename templates for series and films (RD-1100-08); `None` = no sorting. Not
    /// inherited: sorting is a property of the category, there is no global template.
    #[serde(default)]
    pub sorting: Option<crate::SortTemplates>,
    /// Whether packages in this category unwrap a single folder named like the package;
    /// `None` = global default (RD-1140-01).
    #[serde(default)]
    pub unwrap_package_folder: Option<bool>,
    /// Which package-name rules a new package of this category gets (RD-1140-05); `None`, and
    /// every unset switch inside, inherits the global setting.
    #[serde(default)]
    pub package_name_rules: Option<crate::PackageNameRulesOverride>,
    /// Regex find → replace pairs for new package names of this category (RD-1140-05); `None`
    /// inherits the global list, a list — an empty one too — replaces it.
    #[serde(default)]
    pub package_name_regex: Option<Vec<crate::PackageNameRegex>>,
    /// The download window of this category's packages (RD-1240-30); a package's own wins.
    /// `None` leaves them to the bandwidth schedule alone. Set on its own route only.
    #[serde(default)]
    pub download_window: Option<crate::DownloadWindow>,
}

/// Allowlisted filesystem root available to categories.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StorageRootConfig {
    pub id: StorageRootId,
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// Free space that must remain on this root after a download finishes; `None`
    /// inherits `storage_minimum_free_bytes` from the service settings.
    #[serde(default)]
    pub minimum_free_bytes: Option<crate::ByteCount>,
}

/// Prioritized first-match category rule.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CategoryRule {
    pub id: CategoryRuleId,
    pub name: String,
    pub priority: i32,
    pub source: Option<IngressSource>,
    pub domain: Option<String>,
    pub protocol: Option<String>,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub name_regex: Option<String>,
    /// Which name `name_regex` is matched against (RD-1140-02); a rule stored before the
    /// choice existed matches the file name, as it always did.
    #[serde(default)]
    pub name_target: CategoryRuleNameTarget,
    pub category_id: CategoryId,
    pub enabled: bool,
}

/// The name a category rule's `name_regex` is matched against.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CategoryRuleNameTarget {
    /// The link's file name.
    #[default]
    File,
    /// The name of the package the link is grouped into; for an NZB, the NZB's name.
    Package,
    /// Either of the two; one matching is enough.
    Either,
}

/// Location responsible for watching a hotfolder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HotFolderExecutor {
    Daemon,
    CaptureAgent { agent_id: CaptureAgentId },
}

/// Whether an intake source waits for review or directly enters the queue.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    Review,
    Enqueue,
}

/// Persisted hotfolder configuration.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct HotFolderConfig {
    pub id: HotFolderId,
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

#[cfg(test)]
mod tests {
    use super::{CategoryRule, CategoryRuleNameTarget};
    use crate::{CategoryId, CategoryRuleId};

    /// RD-1140-02: a rule as an older client or backup carries it, without `name_target`,
    /// targets the file name -- the only name a rule was ever matched against.
    #[test]
    fn a_rule_without_a_name_target_matches_the_file_name() {
        let stored = serde_json::json!({
            "id": CategoryRuleId::new(),
            "name": "older",
            "priority": 1,
            "source": null,
            "domain": null,
            "protocol": null,
            "extension": null,
            "mime_type": null,
            "name_regex": "x",
            "category_id": CategoryId::new(),
            "enabled": true,
        });
        let rule: CategoryRule = serde_json::from_value(stored).expect("rule");
        assert_eq!(rule.name_target, CategoryRuleNameTarget::File);
        assert_eq!(
            serde_json::to_value(CategoryRuleNameTarget::Either).expect("value"),
            serde_json::json!("either")
        );
    }
}

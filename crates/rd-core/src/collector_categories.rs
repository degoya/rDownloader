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
    pub category_id: CategoryId,
    pub enabled: bool,
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

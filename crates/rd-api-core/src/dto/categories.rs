//! Categories, their post-processing, category rules and hot folders.

use super::*;

#[derive(Deserialize, ToSchema)]
pub struct CreateCategoryRequest {
    pub name: String,
    pub color: String,
    pub storage_root_id: rd_core::StorageRootId,
    pub relative_path: String,
    pub is_default: bool,
    /// Default post-processing level for packages of this category (`null` = global default).
    #[serde(default)]
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    /// Default post-processing script (file name inside the scripts directory).
    #[serde(default)]
    pub script: Option<String>,
    /// Extensions removed after unpacking this category (`null` = global cleanup list).
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    /// Whether packages of this category unpack nested archives recursively (`null` = global default).
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    /// Whether packages of this category unpack every archive set into a folder of its own,
    /// named after the archive (`null` = global default, RD-170-16).
    #[serde(default)]
    pub unpack_to_subfolder: Option<bool>,
    /// Whether packages of this category unpack a multi-volume RAR set while it still
    /// downloads (`null` = global default, RD-1100-07).
    #[serde(default)]
    pub direct_unpack: Option<bool>,
    /// Whether packages of this category are scanned by ClamAV before they count as finished
    /// (`null` = global default, RD-190-14).
    #[serde(default)]
    pub malware_scan: Option<bool>,
    /// Whether packages of this category verify `.sfv` checksums (`null` = global default).
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    /// Whether a failed PAR2/SFV/RAR verification blocks unpacking and everything after it
    /// (`null` = global default). Off means the unpack runs anyway (RD-104-04).
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    /// Whether packages of this category discard the PAR2 recovery set after a successful
    /// unpack (`null` = global default).
    #[serde(default)]
    pub delete_par2: Option<bool>,
    /// Whether packages of this category upload to rclone (`null` = global default).
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    /// rclone target override in `remote:path` form (`null` = the global remote).
    #[serde(default)]
    pub upload_remote: Option<String>,
    /// Whether packages of this category move the content of a single folder named like the
    /// package up into the package folder and remove it (`null` = global default, RD-1140-01).
    #[serde(default)]
    pub unwrap_package_folder: Option<bool>,
}

/// Post-processing defaults of a category (`null` = inherit the global setting).
#[derive(Deserialize, ToSchema)]
pub struct CategoryPostprocessRequest {
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    pub script: Option<String>,
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    /// Whether packages of this category unpack nested archives recursively (`null` = global default).
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    /// Whether packages of this category unpack every archive set into a folder of its own,
    /// named after the archive (`null` = global default, RD-170-16).
    #[serde(default)]
    pub unpack_to_subfolder: Option<bool>,
    /// Whether packages of this category unpack a multi-volume RAR set while it still
    /// downloads (`null` = global default, RD-1100-07).
    #[serde(default)]
    pub direct_unpack: Option<bool>,
    /// Whether packages of this category are scanned by ClamAV before they count as finished
    /// (`null` = global default, RD-190-14).
    #[serde(default)]
    pub malware_scan: Option<bool>,
    /// Whether packages of this category verify `.sfv` checksums (`null` = global default).
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    /// Whether a failed PAR2/SFV/RAR verification blocks unpacking and everything after it
    /// (`null` = global default). Off means the unpack runs anyway (RD-104-04).
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    /// Whether packages of this category discard the PAR2 recovery set after a successful
    /// unpack (`null` = global default).
    #[serde(default)]
    pub delete_par2: Option<bool>,
    /// Post-processing plugin steps for this category, by plugin id and in the order they
    /// run (`null` = the global list). An empty list means "none here", which is how a
    /// category switches a globally enabled step off.
    #[serde(default)]
    pub plugin_steps: Option<Vec<String>>,
    /// Whether packages of this category upload to rclone (`null` = global default).
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    /// rclone target override in `remote:path` form (`null` = the global remote).
    #[serde(default)]
    pub upload_remote: Option<String>,
    /// Sort and rename templates for series, dated episodes and films (RD-1100-08); `null`, or
    /// every template blank, means no sorting. Not inherited: there is no global template.
    #[serde(default)]
    pub sorting: Option<rd_core::SortTemplates>,
    /// Whether packages of this category move the content of a single folder named like the
    /// package up into the package folder and remove it (`null` = global default, RD-1140-01).
    #[serde(default)]
    pub unwrap_package_folder: Option<bool>,
    /// Package-name rules of this category (RD-1140-05); `null`, and every switch left out or
    /// `null`, inherits the global setting.
    #[serde(default)]
    pub package_name_rules: Option<rd_core::PackageNameRulesOverride>,
    /// Regex pairs of this category (RD-1140-05); `null` inherits the global list, a list —
    /// an empty one too — replaces it. Same limits and codes as the global list.
    #[serde(default)]
    pub package_name_regex: Option<Vec<rd_core::PackageNameRegex>>,
}

/// Most example names one preview expands; the rest of a longer list is left out.
pub const MAX_SORT_PREVIEW_NAMES: usize = 20;

/// The templates of a category editor, saved or not, and the names to try them on.
#[derive(Deserialize, ToSchema)]
pub struct SortPreviewRequest {
    pub sorting: rd_core::SortTemplates,
    /// File, folder or package names, at most 20; each is recognised as the sort would.
    pub names: Vec<String>,
}

/// What the sort would make of one name.
#[derive(Serialize, ToSchema)]
pub struct SortPreviewEntry {
    pub name: String,
    /// What the name was recognised as; `null` when it was not, and the file stays unchanged.
    pub kind: Option<rd_core::SortKind>,
    /// The values a template gets for this name, by field.
    pub fields: std::collections::BTreeMap<String, String>,
    /// Where the file would land, below the category's folder, `/` between folders; `null`
    /// when it stays where it is.
    pub path: Option<String>,
    /// Why a recognised name stays: `sort.no_template`, or the code of the template error the
    /// name ran into.
    pub code: Option<String>,
}

/// The preview of every name, and the fields each kind's template may use.
#[derive(Serialize, ToSchema)]
pub struct SortPreviewResponse {
    pub entries: Vec<SortPreviewEntry>,
    /// `series`, `dated` and `movie`, each with its fields.
    pub fields: std::collections::BTreeMap<String, Vec<String>>,
}

/// Longest example name a package-name preview reads; the rest of a longer one is cut off.
pub const MAX_PACKAGE_NAME_PREVIEW_CHARS: usize = 500;

/// An example name and the rules to try on it (RD-1140-05).
#[derive(Deserialize, ToSchema)]
pub struct PackageNamePreviewRequest {
    pub name: String,
    /// The switches of a form, saved or not; `null`, and every switch left out or `null`,
    /// takes the saved global setting.
    #[serde(default)]
    pub rules: Option<rd_core::PackageNameRulesOverride>,
    /// The regex pairs of a form, saved or not; `null` takes the saved global list.
    #[serde(default)]
    pub regex: Option<Vec<rd_core::PackageNameRegex>>,
}

/// What a new package of that name would be called, and the folder it would get.
#[derive(Serialize, ToSchema)]
pub struct PackageNamePreviewResponse {
    pub name: String,
    pub folder: String,
    /// The rules in force for the preview, inherited switches filled in.
    pub rules: rd_core::PackageNameRules,
}

/// One package currently in (or waiting for) the post-processing pipeline.
#[derive(Serialize, ToSchema)]
pub struct PostprocessQueueEntry {
    pub package_id: rd_core::PackageId,
    pub name: String,
    pub state: rd_core::PackageState,
    pub stage: Option<rd_core::PostprocessStage>,
    pub percent: Option<u8>,
    pub current: Option<String>,
    /// `true` while the job waits for the single post-processing worker.
    pub pending: bool,
}

/// Script files available in the configured scripts directory.
#[derive(Serialize, ToSchema)]
pub struct PostprocessScriptsResponse {
    pub directory: String,
    pub scripts: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateCategoryRuleRequest {
    pub name: String,
    pub priority: i32,
    pub source: Option<rd_core::IngressSource>,
    pub domain: Option<String>,
    pub protocol: Option<String>,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub name_regex: Option<String>,
    /// The name `name_regex` is matched against; omitted means the file name (RD-1140-02).
    #[serde(default)]
    pub name_target: rd_core::CategoryRuleNameTarget,
    pub category_id: rd_core::CategoryId,
    pub enabled: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateHotFolderRequest {
    pub name: String,
    pub executor: rd_core::HotFolderExecutor,
    pub path: String,
    pub recursive: bool,
    pub category_id: Option<rd_core::CategoryId>,
    pub import_mode: rd_core::ImportMode,
    pub processed_path: String,
    pub failed_path: String,
    pub enabled: bool,
}

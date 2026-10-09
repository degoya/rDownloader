//! The commands of `writer/config.rs`.

use rd_core::{Category, CategoryRule, HotFolderConfig, StorageRootConfig};

use super::Reply;
use crate::config_store::{NewCategory, NewCategoryRule, NewHotFolder, NewStorageRoot};

/// The commands `Writer::handle_config` applies.
pub(crate) enum ConfigCommand {
    /// Replaces the seeding override of one category.
    SetCategorySeedingPolicy {
        id: rd_core::CategoryId,
        policy: Option<rd_core::SeedingPolicyOverride>,
        reply: Reply<()>,
    },
    UpdateCategoryPostprocess {
        id: rd_core::CategoryId,
        postprocess: crate::CategoryPostprocess,
        reply: Reply<rd_core::Category>,
    },
    CreateStorageRoot {
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
        reply: Reply<StorageRootConfig>,
    },
    CreateCategory {
        input: NewCategory,
        reply: Reply<Category>,
    },
    CreateCategoryRule {
        input: NewCategoryRule,
        reply: Reply<CategoryRule>,
    },
    CreateHotFolder {
        input: NewHotFolder,
        reply: Reply<HotFolderConfig>,
    },
    UpdateStorageRoot {
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
        reply: Reply<StorageRootConfig>,
    },
    DeleteStorageRoot {
        id: rd_core::StorageRootId,
        reply: Reply<()>,
    },
    UpdateCategory {
        id: rd_core::CategoryId,
        input: NewCategory,
        reply: Reply<Category>,
    },
    DeleteCategory {
        id: rd_core::CategoryId,
        reply: Reply<()>,
    },
    UpdateCategoryRule {
        id: rd_core::CategoryRuleId,
        input: NewCategoryRule,
        reply: Reply<CategoryRule>,
    },
    DeleteCategoryRule {
        id: rd_core::CategoryRuleId,
        reply: Reply<()>,
    },
    UpdateHotFolder {
        id: rd_core::HotFolderId,
        input: NewHotFolder,
        reply: Reply<HotFolderConfig>,
    },
    DeleteHotFolder {
        id: rd_core::HotFolderId,
        reply: Reply<()>,
    },
    /// Writes a user site rule, replacing an earlier one of the same id (RD-110-04).
    UpsertSiteRule {
        input: crate::NewUserSiteRule,
        reply: Reply<crate::UserSiteRule>,
    },
    /// Removes a user site rule; answers whether one was there.
    DeleteSiteRule { id: String, reply: Reply<bool> },
    /// Removes every user site rule and every self-test result; answers how many rules went
    /// (RD-1230-03).
    DeleteAllSiteRules { reply: Reply<u64> },
    /// Writes the results of one rule self-test run (RD-110-09).
    /// Switches one shipped rule or one group off or on (RD-110-08).
    SetSiteRuleSwitch {
        scope: String,
        key: String,
        enabled: bool,
        reply: Reply<()>,
    },
    RecordSiteRuleChecks {
        checks: Vec<crate::NewSiteRuleCheck>,
        reply: Reply<()>,
    },
}

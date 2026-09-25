use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{
    Category, CategoryId, CategoryRule, CategoryRuleId, EventEnvelope, EventKind, HotFolderConfig,
    HotFolderExecutor, HotFolderId, ImportMode, IngressSource, StorageRootConfig, StorageRootId,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, parse_id, writer::insert_event};

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
    pub sfv_verify: Option<bool>,
    pub safe_postproc: Option<bool>,
    pub delete_par2: Option<bool>,
    pub upload_enabled: Option<bool>,
    pub upload_remote: Option<String>,
}

/// A category's post-processing overrides; `None` per field inherits the global setting.
///
/// These seven values are only ever read and written together, so they travel as one value
/// instead of as seven positional parameters that are easy to transpose at a call site.
#[derive(Clone, Debug, Default)]
pub struct CategoryPostprocess {
    pub level: Option<rd_core::PostprocessLevel>,
    pub script: Option<String>,
    pub cleanup_extensions: Option<Vec<String>>,
    pub recursive_unpack: Option<bool>,
    pub sfv_verify: Option<bool>,
    pub safe_postproc: Option<bool>,
    pub delete_par2: Option<bool>,
    /// Plugin steps for this category, by plugin id; `None` inherits, an empty list means
    /// "none here" and switches a globally enabled step off.
    pub plugin_steps: Option<Vec<String>>,
    pub upload_enabled: Option<bool>,
    pub upload_remote: Option<String>,
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

pub(crate) async fn create_storage_root(
    connection: &mut SqliteConnection,
    id: StorageRootId,
    input: NewStorageRoot,
) -> Result<(StorageRootConfig, EventEnvelope)> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    // The first root is always the default. An install without one leaves destination
    // resolution with nothing to fall back to, and the flag is easy to miss in the form.
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage_roots")
        .fetch_one(&mut *tx)
        .await?;
    let value = StorageRootConfig {
        id,
        name: input.name,
        path: input.path,
        is_default: input.is_default || existing == 0,
        minimum_free_bytes: input.minimum_free_bytes,
    };
    let event = config_event(EventKind::CategoryChanged, "storage_root", value.id);
    // Clear the old default before inserting the new one: `idx_storage_roots_single_default`
    // rejects two default rows even mid-transaction, so the order is load-bearing.
    if value.is_default {
        sqlx::query("UPDATE storage_roots SET is_default = 0, updated_at = ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO storage_roots (id, name, path, is_default, minimum_free_bytes, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(&value.path)
    .bind(value.is_default)
    .bind(value.minimum_free_bytes.map(persisted_bytes))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn create_category(
    connection: &mut SqliteConnection,
    input: NewCategory,
) -> Result<(Category, EventEnvelope)> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    // The first category is always the default. Routing falls back to it for every link no
    // rule matched, so an install without one drops those links into no category at all, and
    // the flag is easy to miss in the form.
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories")
        .fetch_one(&mut *tx)
        .await?;
    let value = Category {
        id: CategoryId::new(),
        name: input.name,
        color: input.color,
        storage_root_id: input.storage_root_id,
        relative_path: input.relative_path,
        is_default: input.is_default || existing == 0,
        postprocess_level: input.postprocess_level,
        script: input.script,
        cleanup_extensions: input.cleanup_extensions,
        recursive_unpack: input.recursive_unpack,
        sfv_verify: input.sfv_verify,
        safe_postproc: input.safe_postproc,
        delete_par2: input.delete_par2,
        upload_enabled: input.upload_enabled,
        upload_remote: input.upload_remote,
        // A new category inherits the global seeding policy and plugin steps.
        seeding: None,
        plugin_steps: None,
    };
    let event = config_event(EventKind::CategoryChanged, "category", value.id);
    // Clear the old default before inserting the new one: `idx_categories_single_default`
    // rejects two default rows even mid-transaction, so the order is load-bearing.
    if value.is_default {
        sqlx::query("UPDATE categories SET is_default = 0, updated_at = ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO categories (id, name, color, storage_root_id, relative_path, is_default, postprocess_level, script, cleanup_extensions, recursive_unpack, sfv_verify, safe_postproc, delete_par2, upload_enabled, upload_remote, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(value.id.to_string())
        .bind(&value.name)
        .bind(&value.color)
        .bind(value.storage_root_id.to_string())
        .bind(&value.relative_path)
        .bind(value.is_default)
        .bind(value.postprocess_level.map(crate::writer::level_string))
        .bind(&value.script)
        .bind(cleanup_json(value.cleanup_extensions.as_ref())?)
        .bind(value.recursive_unpack)
        .bind(value.sfv_verify)
        .bind(value.safe_postproc)
        .bind(value.delete_par2)
        .bind(value.upload_enabled)
        .bind(&value.upload_remote)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn create_category_rule(
    connection: &mut SqliteConnection,
    input: NewCategoryRule,
) -> Result<(CategoryRule, EventEnvelope)> {
    let value = CategoryRule {
        id: CategoryRuleId::new(),
        name: input.name,
        priority: input.priority,
        source: input.source,
        domain: input.domain,
        protocol: input.protocol,
        extension: input.extension,
        mime_type: input.mime_type,
        name_regex: input.name_regex,
        category_id: input.category_id,
        enabled: input.enabled,
    };
    let now = Utc::now();
    let event = config_event(EventKind::CategoryChanged, "category_rule", value.id);
    let source = value.source.map(enum_string).transpose()?;
    let mut tx = connection.begin().await?;
    sqlx::query("INSERT INTO category_rules (id, name, priority, source, domain, protocol, extension, mime_type, name_regex, category_id, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(value.id.to_string())
        .bind(&value.name)
        .bind(value.priority)
        .bind(source)
        .bind(&value.domain)
        .bind(&value.protocol)
        .bind(&value.extension)
        .bind(&value.mime_type)
        .bind(&value.name_regex)
        .bind(value.category_id.to_string())
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn create_hotfolder(
    connection: &mut SqliteConnection,
    input: NewHotFolder,
) -> Result<(HotFolderConfig, EventEnvelope)> {
    let value = HotFolderConfig {
        id: HotFolderId::new(),
        name: input.name,
        executor: input.executor,
        path: input.path,
        recursive: input.recursive,
        category_id: input.category_id,
        import_mode: input.import_mode,
        processed_path: input.processed_path,
        failed_path: input.failed_path,
        enabled: input.enabled,
    };
    let now = Utc::now();
    let event = config_event(EventKind::HotFolderChanged, "hotfolder", value.id);
    let mut tx = connection.begin().await?;
    sqlx::query("INSERT INTO hotfolders (id, name, executor_json, path, recursive, category_id, import_mode, processed_path, failed_path, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(value.id.to_string())
        .bind(&value.name)
        .bind(serde_json::to_string(&value.executor)?)
        .bind(&value.path)
        .bind(value.recursive)
        .bind(value.category_id.map(|id| id.to_string()))
        .bind(enum_string(value.import_mode)?)
        .bind(&value.processed_path)
        .bind(&value.failed_path)
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

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

/// The root marked default. Exactly one exists whenever the table is non-empty; the write
/// paths in this module maintain that, so callers need no alphabetical fallback.
pub(crate) async fn default_storage_root(pool: &SqlitePool) -> Result<Option<StorageRootConfig>> {
    sqlx::query_as::<_, StorageRootRow>(
        "SELECT id, name, path, is_default, minimum_free_bytes FROM storage_roots \
         WHERE is_default = 1 LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn list_storage_roots(pool: &SqlitePool) -> Result<Vec<StorageRootConfig>> {
    sqlx::query_as::<_, StorageRootRow>(
        "SELECT id, name, path, is_default, minimum_free_bytes FROM storage_roots \
         ORDER BY is_default DESC, name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

const CATEGORY_COLUMNS: &str = "id, name, color, storage_root_id, relative_path, is_default, postprocess_level, script, cleanup_extensions, recursive_unpack, sfv_verify, safe_postproc, delete_par2, upload_enabled, upload_remote, seeding_json, plugin_steps_json";
const RULE_COLUMNS: &str = "id, name, priority, source, domain, protocol, extension, mime_type, name_regex, category_id, enabled";
const HOTFOLDER_COLUMNS: &str = "id, name, executor_json, path, recursive, category_id, import_mode, processed_path, failed_path, enabled";

pub(crate) async fn list_categories(pool: &SqlitePool) -> Result<Vec<Category>> {
    sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM categories ORDER BY is_default DESC, name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn list_category_rules(pool: &SqlitePool) -> Result<Vec<CategoryRule>> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {RULE_COLUMNS} FROM category_rules ORDER BY priority, name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn list_hotfolders(pool: &SqlitePool) -> Result<Vec<HotFolderConfig>> {
    sqlx::query_as::<_, HotFolderRow>(&format!(
        "SELECT {HOTFOLDER_COLUMNS} FROM hotfolders ORDER BY name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn routing_config(
    connection: &mut SqliteConnection,
) -> Result<(Vec<CategoryRule>, Option<CategoryId>)> {
    let rules = sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {RULE_COLUMNS} FROM category_rules ORDER BY priority, name"
    ))
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect::<Result<Vec<_>>>()?;
    let default = sqlx::query_scalar::<_, String>(
        "SELECT id FROM categories WHERE is_default = 1 ORDER BY updated_at DESC LIMIT 1",
    )
    .fetch_optional(&mut *connection)
    .await?
    .as_deref()
    .map(parse_id)
    .transpose()?;
    Ok((rules, default))
}

/// `ByteCount` is bounded by SQLite's INTEGER range when it is constructed, so the cast
/// cannot lose data; the saturating fallback only keeps the lint about panics happy.
fn persisted_bytes(value: rd_core::ByteCount) -> i64 {
    i64::try_from(value.get()).unwrap_or(i64::MAX)
}

#[derive(FromRow)]
struct StorageRootRow {
    id: String,
    name: String,
    path: String,
    is_default: bool,
    minimum_free_bytes: Option<i64>,
}
impl TryFrom<StorageRootRow> for StorageRootConfig {
    type Error = anyhow::Error;
    fn try_from(row: StorageRootRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            path: row.path,
            is_default: row.is_default,
            minimum_free_bytes: row
                .minimum_free_bytes
                .and_then(|value| u64::try_from(value).ok())
                .map(rd_core::ByteCount::new)
                .transpose()
                .map_err(|error| anyhow::anyhow!(error))?,
        })
    }
}

#[derive(FromRow)]
struct CategoryRow {
    id: String,
    name: String,
    color: String,
    storage_root_id: String,
    relative_path: String,
    is_default: bool,
    postprocess_level: Option<String>,
    script: Option<String>,
    cleanup_extensions: Option<String>,
    recursive_unpack: Option<bool>,
    sfv_verify: Option<bool>,
    safe_postproc: Option<bool>,
    delete_par2: Option<bool>,
    upload_enabled: Option<bool>,
    upload_remote: Option<String>,
    seeding_json: Option<String>,
    plugin_steps_json: Option<String>,
}
impl TryFrom<CategoryRow> for Category {
    type Error = anyhow::Error;
    fn try_from(row: CategoryRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            color: row.color,
            storage_root_id: parse_id(&row.storage_root_id)?,
            relative_path: row.relative_path,
            is_default: row.is_default,
            postprocess_level: crate::models::parse_level(row.postprocess_level.as_deref()),
            script: row.script,
            // A malformed blob must not hide the whole category; fall back to inheriting.
            cleanup_extensions: row
                .cleanup_extensions
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok()),
            recursive_unpack: row.recursive_unpack,
            sfv_verify: row.sfv_verify,
            safe_postproc: row.safe_postproc,
            delete_par2: row.delete_par2,
            upload_enabled: row.upload_enabled,
            upload_remote: row.upload_remote,
            // A malformed blob must not hide the category; fall back to inheriting.
            seeding: row
                .seeding_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok()),
            plugin_steps: row
                .plugin_steps_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok()),
        })
    }
}

#[derive(FromRow)]
struct RuleRow {
    id: String,
    name: String,
    priority: i32,
    source: Option<String>,
    domain: Option<String>,
    protocol: Option<String>,
    extension: Option<String>,
    mime_type: Option<String>,
    name_regex: Option<String>,
    category_id: String,
    enabled: bool,
}
impl TryFrom<RuleRow> for CategoryRule {
    type Error = anyhow::Error;
    fn try_from(row: RuleRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            priority: row.priority,
            source: row.source.map(|value| parse_enum(&value)).transpose()?,
            domain: row.domain,
            protocol: row.protocol,
            extension: row.extension,
            mime_type: row.mime_type,
            name_regex: row.name_regex,
            category_id: parse_id(&row.category_id)?,
            enabled: row.enabled,
        })
    }
}

#[derive(FromRow)]
struct HotFolderRow {
    id: String,
    name: String,
    executor_json: String,
    path: String,
    recursive: bool,
    category_id: Option<String>,
    import_mode: String,
    processed_path: String,
    failed_path: String,
    enabled: bool,
}
impl TryFrom<HotFolderRow> for HotFolderConfig {
    type Error = anyhow::Error;
    fn try_from(row: HotFolderRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            executor: serde_json::from_str(&row.executor_json)?,
            path: row.path,
            recursive: row.recursive,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            import_mode: parse_enum(&row.import_mode)?,
            processed_path: row.processed_path,
            failed_path: row.failed_path,
            enabled: row.enabled,
        })
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

pub(crate) async fn update_storage_root(
    connection: &mut SqliteConnection,
    id: StorageRootId,
    input: NewStorageRoot,
) -> Result<(StorageRootConfig, EventEnvelope)> {
    let now = Utc::now();
    let event = config_event(EventKind::CategoryChanged, "storage_root", id);
    let mut tx = connection.begin().await?;
    // Giving up the last default is not a thing a user can do: something has to stay the
    // fallback. Coerce rather than reject, and let the response tell the form what happened.
    let holds_only_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM storage_roots WHERE id = ? AND is_default = 1) \
         AND (SELECT COUNT(*) FROM storage_roots WHERE is_default = 1) <= 1",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let value = StorageRootConfig {
        id,
        name: input.name,
        path: input.path,
        is_default: input.is_default || holds_only_default,
        minimum_free_bytes: input.minimum_free_bytes,
    };
    if value.is_default {
        sqlx::query("UPDATE storage_roots SET is_default = 0, updated_at = ? WHERE id != ?")
            .bind(now)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    let updated = sqlx::query(
        "UPDATE storage_roots SET name = ?, path = ?, is_default = ?, minimum_free_bytes = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(&value.path)
    .bind(value.is_default)
    .bind(value.minimum_free_bytes.map(persisted_bytes))
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("storage root not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_storage_root(
    connection: &mut SqliteConnection,
    id: StorageRootId,
) -> Result<EventEnvelope> {
    let event = config_event(EventKind::CategoryChanged, "storage_root", id);
    let mut tx = connection.begin().await?;
    // `categories.storage_root_id` is NOT NULL, so a referenced root cannot be removed
    // without orphaning categories. Report it instead of letting the FK error surface.
    let categories: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE storage_root_id = ?")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
    anyhow::ensure!(
        categories == 0,
        StoreError::in_use(format!(
            "storage root is still used by {categories} category(ies)"
        ))
    );
    let was_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM storage_roots WHERE id = ? AND is_default = 1)",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let deleted = sqlx::query("DELETE FROM storage_roots WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("storage root not found"));
    }
    // Promote after the delete, never before: the partial unique index would otherwise see
    // two defaults. A no-op when that root was the last one.
    if was_default {
        sqlx::query(
            "UPDATE storage_roots SET is_default = 1, updated_at = ? \
             WHERE id = (SELECT id FROM storage_roots ORDER BY name LIMIT 1)",
        )
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn update_category(
    connection: &mut SqliteConnection,
    id: CategoryId,
    input: NewCategory,
) -> Result<(Category, EventEnvelope)> {
    let now = Utc::now();
    let event = config_event(EventKind::CategoryChanged, "category", id);
    let mut tx = connection.begin().await?;
    // Giving up the last default is not a thing a user can do: routing has to keep a fallback.
    // Coerce rather than reject, and let the response tell the form what happened.
    let holds_only_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM categories WHERE id = ? AND is_default = 1) \
         AND (SELECT COUNT(*) FROM categories WHERE is_default = 1) <= 1",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let value = Category {
        id,
        name: input.name,
        color: input.color,
        storage_root_id: input.storage_root_id,
        relative_path: input.relative_path,
        is_default: input.is_default || holds_only_default,
        postprocess_level: input.postprocess_level,
        script: input.script,
        cleanup_extensions: input.cleanup_extensions,
        recursive_unpack: input.recursive_unpack,
        sfv_verify: input.sfv_verify,
        safe_postproc: input.safe_postproc,
        delete_par2: input.delete_par2,
        upload_enabled: input.upload_enabled,
        upload_remote: input.upload_remote,
        // A new category inherits the global seeding policy and plugin steps.
        seeding: None,
        plugin_steps: None,
    };
    if value.is_default {
        sqlx::query("UPDATE categories SET is_default = 0, updated_at = ? WHERE id != ?")
            .bind(now)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    let updated = sqlx::query(
        "UPDATE categories SET name = ?, color = ?, storage_root_id = ?, relative_path = ?, \
         is_default = ?, postprocess_level = ?, script = ?, cleanup_extensions = ?, \
         recursive_unpack = ?, sfv_verify = ?, safe_postproc = ?, delete_par2 = ?, upload_enabled = ?, upload_remote = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(&value.color)
    .bind(value.storage_root_id.to_string())
    .bind(&value.relative_path)
    .bind(value.is_default)
    .bind(value.postprocess_level.map(crate::writer::level_string))
    .bind(&value.script)
    .bind(cleanup_json(value.cleanup_extensions.as_ref())?)
    .bind(value.recursive_unpack)
    .bind(value.sfv_verify)
    .bind(value.safe_postproc)
    .bind(value.delete_par2)
    .bind(value.upload_enabled)
    .bind(&value.upload_remote)
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// Replaces the seeding override of one category; `None` clears it back to inheriting.
pub(crate) async fn set_category_seeding(
    connection: &mut SqliteConnection,
    id: rd_core::CategoryId,
    policy: Option<rd_core::SeedingPolicyOverride>,
) -> Result<rd_core::EventEnvelope> {
    // An override that sets nothing is stored as "inherit", so the two never diverge.
    let stored = policy
        .filter(|policy| !policy.is_empty())
        .map(|policy| serde_json::to_string(&policy))
        .transpose()?;
    let affected = sqlx::query("UPDATE categories SET seeding_json = ? WHERE id = ?")
        .bind(stored)
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?
        .rows_affected();
    anyhow::ensure!(affected == 1, StoreError::not_found("category not found"));
    Ok(rd_core::EventEnvelope::new(
        rd_core::EventKind::CategoryChanged,
        serde_json::json!({ "category_id": id }),
    ))
}

pub(crate) async fn delete_category(
    connection: &mut SqliteConnection,
    id: CategoryId,
) -> Result<EventEnvelope> {
    let event = config_event(EventKind::CategoryChanged, "category", id);
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    // Packages carry the category as a plain column (no foreign key), so nothing stops the
    // delete at the database level. Running packages still need their category to resolve a
    // destination, hence the explicit guard; finished ones only lose a label.
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM packages WHERE category_id = ? AND state NOT IN ('completed', 'failed')",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    anyhow::ensure!(
        active == 0,
        StoreError::in_use(format!(
            "category is still used by {active} unfinished package(s)"
        ))
    );
    sqlx::query("UPDATE packages SET category_id = NULL, updated_at = ? WHERE category_id = ?")
        .bind(now)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    // `category_rules.category_id` is NOT NULL: a rule without its category is meaningless,
    // so the rules go with it.
    sqlx::query("DELETE FROM category_rules WHERE category_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    // `link_candidates.category_id` is a real foreign key (0001_initial) with no ON DELETE,
    // and `foreign_keys` is on, so leaving it out did not merely dangle — the whole delete
    // aborted with a raw SQLite constraint error instead of one of the stable REST codes
    // whenever the LinkGrabber held a candidate in this category. `subscriptions` and
    // `stream_channels` carry no key at all and kept routing to a category that was gone.
    for table in [
        "hotfolders",
        "nzb_imports",
        "collector_packages",
        "link_candidates",
        "subscriptions",
        "stream_channels",
    ] {
        sqlx::query(&format!(
            "UPDATE {table} SET category_id = NULL WHERE category_id = ?"
        ))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    }
    let was_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM categories WHERE id = ? AND is_default = 1)",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let deleted = sqlx::query("DELETE FROM categories WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category not found"));
    }
    // Promote after the delete, never before: the partial unique index would otherwise see
    // two defaults. A no-op when that category was the last one.
    if was_default {
        sqlx::query(
            "UPDATE categories SET is_default = 1, updated_at = ? \
             WHERE id = (SELECT id FROM categories ORDER BY name LIMIT 1)",
        )
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn update_category_rule(
    connection: &mut SqliteConnection,
    id: CategoryRuleId,
    input: NewCategoryRule,
) -> Result<(CategoryRule, EventEnvelope)> {
    let value = CategoryRule {
        id,
        name: input.name,
        priority: input.priority,
        source: input.source,
        domain: input.domain,
        protocol: input.protocol,
        extension: input.extension,
        mime_type: input.mime_type,
        name_regex: input.name_regex,
        category_id: input.category_id,
        enabled: input.enabled,
    };
    let event = config_event(EventKind::CategoryChanged, "category_rule", id);
    let source = value.source.map(enum_string).transpose()?;
    let mut tx = connection.begin().await?;
    let updated = sqlx::query(
        "UPDATE category_rules SET name = ?, priority = ?, source = ?, domain = ?, protocol = ?, \
         extension = ?, mime_type = ?, name_regex = ?, category_id = ?, enabled = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(&value.name)
    .bind(value.priority)
    .bind(source)
    .bind(&value.domain)
    .bind(&value.protocol)
    .bind(&value.extension)
    .bind(&value.mime_type)
    .bind(&value.name_regex)
    .bind(value.category_id.to_string())
    .bind(value.enabled)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category rule not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_category_rule(
    connection: &mut SqliteConnection,
    id: CategoryRuleId,
) -> Result<EventEnvelope> {
    let event = config_event(EventKind::CategoryChanged, "category_rule", id);
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM category_rules WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category rule not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn update_hotfolder(
    connection: &mut SqliteConnection,
    id: HotFolderId,
    input: NewHotFolder,
) -> Result<(HotFolderConfig, EventEnvelope)> {
    let value = HotFolderConfig {
        id,
        name: input.name,
        executor: input.executor,
        path: input.path,
        recursive: input.recursive,
        category_id: input.category_id,
        import_mode: input.import_mode,
        processed_path: input.processed_path,
        failed_path: input.failed_path,
        enabled: input.enabled,
    };
    let event = config_event(EventKind::HotFolderChanged, "hotfolder", id);
    let mut tx = connection.begin().await?;
    let updated = sqlx::query(
        "UPDATE hotfolders SET name = ?, executor_json = ?, path = ?, recursive = ?, \
         category_id = ?, import_mode = ?, processed_path = ?, failed_path = ?, enabled = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(serde_json::to_string(&value.executor)?)
    .bind(&value.path)
    .bind(value.recursive)
    .bind(value.category_id.map(|id| id.to_string()))
    .bind(enum_string(value.import_mode)?)
    .bind(&value.processed_path)
    .bind(&value.failed_path)
    .bind(value.enabled)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("hotfolder not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_hotfolder(
    connection: &mut SqliteConnection,
    id: HotFolderId,
) -> Result<EventEnvelope> {
    let event = config_event(EventKind::HotFolderChanged, "hotfolder", id);
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM hotfolders WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("hotfolder not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}

/// Serialises a category's plugin-step list; `None` stays NULL, which means "inherit".
///
/// An empty list is *not* NULL: it is how a category switches a globally enabled step off,
/// and collapsing the two would make that impossible to express.
fn plugin_steps_json(steps: Option<&Vec<String>>) -> Result<Option<String>> {
    steps
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).context("parse stored enum")
}

/// Sets a category's post-processing overrides (`None` = inherit the global setting).
pub(crate) async fn update_category_postprocess(
    connection: &mut SqliteConnection,
    id: CategoryId,
    postprocess: CategoryPostprocess,
) -> Result<(Category, EventEnvelope)> {
    let event = config_event(EventKind::CategoryChanged, "category", id);
    let mut tx = connection.begin().await?;
    let updated = sqlx::query(
        "UPDATE categories SET postprocess_level = ?, script = ?, cleanup_extensions = ?, \
         recursive_unpack = ?, sfv_verify = ?, safe_postproc = ?, delete_par2 = ?, plugin_steps_json = ?, \
         upload_enabled = ?, upload_remote = ?, updated_at = ? WHERE id = ?",
    )
    .bind(postprocess.level.map(crate::writer::level_string))
    .bind(&postprocess.script)
    .bind(cleanup_json(postprocess.cleanup_extensions.as_ref())?)
    .bind(postprocess.recursive_unpack)
    .bind(postprocess.sfv_verify)
    .bind(postprocess.safe_postproc)
    .bind(postprocess.delete_par2)
    .bind(plugin_steps_json(postprocess.plugin_steps.as_ref())?)
    .bind(postprocess.upload_enabled)
    .bind(&postprocess.upload_remote)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let category = sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM categories WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((category, event))
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

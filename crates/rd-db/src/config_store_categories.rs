//! Categories: create, list, edit and delete, and the row they are read from.

use anyhow::Result;
use chrono::Utc;
use rd_core::{Category, CategoryId, EventEnvelope, EventKind};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::{CATEGORY_COLUMNS, CATEGORY_NAME_TAKEN, NewCategory, cleanup_json, config_event};
use crate::{error::StoreError, json_column::lenient, parse_id, writer::insert_event};

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
        unpack_to_subfolder: input.unpack_to_subfolder,
        unwrap_package_folder: input.unwrap_package_folder,
        direct_unpack: input.direct_unpack,
        malware_scan: input.malware_scan,
        sfv_verify: input.sfv_verify,
        safe_postproc: input.safe_postproc,
        delete_par2: input.delete_par2,
        upload_enabled: input.upload_enabled,
        upload_remote: input.upload_remote,
        // A new category inherits the global seeding policy and plugin steps.
        seeding: None,
        plugin_steps: None,
        // Set on the post-processing route only, like the plugin steps.
        sorting: None,
        package_name_rules: None,
        package_name_regex: None,
        // Set on its own route only (RD-1240-30).
        download_window: None,
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
    sqlx::query("INSERT INTO categories (id, name, color, storage_root_id, relative_path, is_default, postprocess_level, script, cleanup_extensions, recursive_unpack, unpack_to_subfolder, direct_unpack, malware_scan, sfv_verify, safe_postproc, delete_par2, upload_enabled, upload_remote, unwrap_package_folder, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(value.id.to_string())
        .bind(&value.name)
        .bind(&value.color)
        .bind(value.storage_root_id.to_string())
        .bind(&value.relative_path)
        .bind(value.is_default)
        .bind(value.postprocess_level.map(crate::enum_string).transpose()?)
        .bind(&value.script)
        .bind(cleanup_json(value.cleanup_extensions.as_ref())?)
        .bind(value.recursive_unpack)
        .bind(value.unpack_to_subfolder)
        .bind(value.direct_unpack)
        .bind(value.malware_scan)
        .bind(value.sfv_verify)
        .bind(value.safe_postproc)
        .bind(value.delete_par2)
        .bind(value.upload_enabled)
        .bind(&value.upload_remote)
        .bind(value.unwrap_package_folder)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| crate::error::tag_duplicate(error, CATEGORY_NAME_TAKEN))?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn list_categories(pool: &SqlitePool) -> Result<Vec<Category>> {
    sqlx::query_as::<_, CategoryRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {CATEGORY_COLUMNS} FROM categories ORDER BY is_default DESC, name"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
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
        unpack_to_subfolder: input.unpack_to_subfolder,
        unwrap_package_folder: input.unwrap_package_folder,
        direct_unpack: input.direct_unpack,
        malware_scan: input.malware_scan,
        sfv_verify: input.sfv_verify,
        safe_postproc: input.safe_postproc,
        delete_par2: input.delete_par2,
        upload_enabled: input.upload_enabled,
        upload_remote: input.upload_remote,
        // A new category inherits the global seeding policy and plugin steps.
        seeding: None,
        plugin_steps: None,
        // Set on the post-processing route only, like the plugin steps.
        sorting: None,
        package_name_rules: None,
        package_name_regex: None,
        // Set on its own route only (RD-1240-30).
        download_window: None,
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
         recursive_unpack = ?, unpack_to_subfolder = ?, direct_unpack = ?, malware_scan = ?, sfv_verify = ?, safe_postproc = ?, delete_par2 = ?, upload_enabled = ?, upload_remote = ?, unwrap_package_folder = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(&value.color)
    .bind(value.storage_root_id.to_string())
    .bind(&value.relative_path)
    .bind(value.is_default)
    .bind(value.postprocess_level.map(crate::enum_string).transpose()?)
    .bind(&value.script)
    .bind(cleanup_json(value.cleanup_extensions.as_ref())?)
    .bind(value.recursive_unpack)
    .bind(value.unpack_to_subfolder)
    .bind(value.direct_unpack)
    .bind(value.malware_scan)
    .bind(value.sfv_verify)
    .bind(value.safe_postproc)
    .bind(value.delete_par2)
    .bind(value.upload_enabled)
    .bind(&value.upload_remote)
    .bind(value.unwrap_package_folder)
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| crate::error::tag_duplicate(error, CATEGORY_NAME_TAKEN))?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
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
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET category_id = NULL WHERE category_id = ?"
        )))
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

#[derive(FromRow)]
pub(super) struct CategoryRow {
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
    unpack_to_subfolder: Option<bool>,
    direct_unpack: Option<bool>,
    malware_scan: Option<bool>,
    sfv_verify: Option<bool>,
    safe_postproc: Option<bool>,
    delete_par2: Option<bool>,
    upload_enabled: Option<bool>,
    upload_remote: Option<String>,
    seeding_json: Option<String>,
    plugin_steps_json: Option<String>,
    sorting_json: Option<String>,
    unwrap_package_folder: Option<bool>,
    package_name_rules_json: Option<String>,
    package_name_regex_json: Option<String>,
    download_window_json: Option<String>,
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
            postprocess_level: crate::models::parse_level(
                row.postprocess_level.as_deref(),
                "categories",
                &row.id,
            ),
            script: row.script,
            // A malformed blob must not hide the whole category; fall back to inheriting.
            cleanup_extensions: row.cleanup_extensions.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "cleanup_extensions",
                    &row.id,
                )
            }),
            recursive_unpack: row.recursive_unpack,
            unpack_to_subfolder: row.unpack_to_subfolder,
            unwrap_package_folder: row.unwrap_package_folder,
            direct_unpack: row.direct_unpack,
            malware_scan: row.malware_scan,
            sfv_verify: row.sfv_verify,
            safe_postproc: row.safe_postproc,
            delete_par2: row.delete_par2,
            upload_enabled: row.upload_enabled,
            upload_remote: row.upload_remote,
            // A malformed blob must not hide the category; fall back to inheriting.
            seeding: row.seeding_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "seeding_json",
                    &row.id,
                )
            }),
            plugin_steps: row.plugin_steps_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "plugin_steps_json",
                    &row.id,
                )
            }),
            // A malformed blob means no sorting rather than a hidden category.
            sorting: row.sorting_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "sorting_json",
                    &row.id,
                )
            }),
            // A malformed blob inherits the global rules rather than hiding the category.
            package_name_rules: row.package_name_rules_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "package_name_rules_json",
                    &row.id,
                )
            }),
            // A malformed list inherits the global one rather than hiding the category.
            package_name_regex: row.package_name_regex_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "package_name_regex_json",
                    &row.id,
                )
            }),
            // A malformed window leaves the packages to the schedule rather than hiding the
            // category (RD-1240-30).
            download_window: row.download_window_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "download_window_json",
                    &row.id,
                )
            }),
        })
    }
}

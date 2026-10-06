//! A category's post-processing overrides, its seeding override and its sort templates.

use anyhow::Result;
use chrono::Utc;
use rd_core::{Category, CategoryId, EventEnvelope, EventKind};
use sqlx::{Connection, SqliteConnection};

use super::{
    CATEGORY_COLUMNS, CategoryPostprocess, categories::CategoryRow, cleanup_json, config_event,
};
use crate::{error::StoreError, writer::insert_event};

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

/// Serialises a category's sort templates; no template at all stays NULL (RD-1100-08).
pub(crate) fn sorting_json(sorting: Option<&rd_core::SortTemplates>) -> Result<Option<String>> {
    sorting
        .cloned()
        .and_then(rd_core::SortTemplates::normalized)
        .map(|templates| serde_json::to_string(&templates))
        .transpose()
        .map_err(Into::into)
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
         recursive_unpack = ?, unpack_to_subfolder = ?, direct_unpack = ?, malware_scan = ?, sfv_verify = ?, safe_postproc = ?, delete_par2 = ?, plugin_steps_json = ?, \
         upload_enabled = ?, upload_remote = ?, sorting_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(postprocess.level.map(crate::enum_string).transpose()?)
    .bind(&postprocess.script)
    .bind(cleanup_json(postprocess.cleanup_extensions.as_ref())?)
    .bind(postprocess.recursive_unpack)
    .bind(postprocess.unpack_to_subfolder)
    .bind(postprocess.direct_unpack)
    .bind(postprocess.malware_scan)
    .bind(postprocess.sfv_verify)
    .bind(postprocess.safe_postproc)
    .bind(postprocess.delete_par2)
    .bind(plugin_steps_json(postprocess.plugin_steps.as_ref())?)
    .bind(postprocess.upload_enabled)
    .bind(&postprocess.upload_remote)
    .bind(sorting_json(postprocess.sorting.as_ref())?)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let category = sqlx::query_as::<_, CategoryRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {CATEGORY_COLUMNS} FROM categories WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((category, event))
}

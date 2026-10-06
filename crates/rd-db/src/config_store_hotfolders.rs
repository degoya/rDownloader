//! Watched folders: create, list, edit and delete.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind, HotFolderConfig, HotFolderId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::{HOTFOLDER_TAKEN, NewHotFolder, config_event};
use crate::{enum_string, error::StoreError, parse_enum, parse_id, writer::insert_event};

const HOTFOLDER_COLUMNS: &str = "id, name, executor_json, path, recursive, category_id, import_mode, processed_path, failed_path, enabled";

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
        .await
        .map_err(|error| crate::error::tag_duplicate(error, HOTFOLDER_TAKEN))?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn list_hotfolders(pool: &SqlitePool) -> Result<Vec<HotFolderConfig>> {
    sqlx::query_as::<_, HotFolderRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {HOTFOLDER_COLUMNS} FROM hotfolders ORDER BY name"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
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
    .await
    .map_err(|error| crate::error::tag_duplicate(error, HOTFOLDER_TAKEN))?;
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

//! Resume state of downloads carried by a transfer plugin.
//!
//! The checkpoint is a blob the backend wrote and only it can read, so nothing here inspects
//! it. What the host does own is which plugin and which version produced it: resuming a
//! checkpoint with a different build would hand a backend somebody else's private format.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, Row, SqliteConnection, SqlitePool};

use rd_core::DownloadId;

/// One running plugin transfer's resume state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginTransfer {
    pub plugin_id: String,
    pub plugin_version: String,
    pub checkpoint: Option<Vec<u8>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct PluginTransferRow {
    plugin_id: String,
    plugin_version: String,
    checkpoint: Option<Vec<u8>>,
    updated_at: DateTime<Utc>,
}

pub(crate) async fn load_plugin_transfer(
    pool: &SqlitePool,
    id: DownloadId,
) -> Result<Option<PluginTransfer>> {
    let row = sqlx::query_as::<_, PluginTransferRow>(
        "SELECT plugin_id, plugin_version, checkpoint, updated_at FROM plugin_transfers \
         WHERE download_id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| PluginTransfer {
        plugin_id: row.plugin_id,
        plugin_version: row.plugin_version,
        checkpoint: row.checkpoint,
        updated_at: row.updated_at,
    }))
}

/// Writes the checkpoint, keeping the version that first claimed the job.
///
/// The pin is claimed rather than overwritten: a job that started on one version finishes on
/// it, so a bundled upgrade landing mid-download cannot hand a half-written file to a build
/// that reads checkpoints differently.
pub(crate) async fn save_plugin_transfer(
    connection: &mut SqliteConnection,
    id: DownloadId,
    plugin_id: &str,
    plugin_version: &str,
    checkpoint: Option<Vec<u8>>,
) -> Result<PluginTransfer> {
    sqlx::query(
        "INSERT INTO plugin_transfers (download_id, plugin_id, plugin_version, checkpoint, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(download_id) DO UPDATE SET checkpoint = excluded.checkpoint, \
         updated_at = excluded.updated_at",
    )
    .bind(id.to_string())
    .bind(plugin_id)
    .bind(plugin_version)
    .bind(checkpoint)
    .bind(Utc::now())
    .execute(&mut *connection)
    .await?;
    let row = sqlx::query(
        "SELECT plugin_id, plugin_version, checkpoint, updated_at FROM plugin_transfers \
         WHERE download_id = ?",
    )
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?;
    Ok(PluginTransfer {
        plugin_id: row.get("plugin_id"),
        plugin_version: row.get("plugin_version"),
        checkpoint: row.get("checkpoint"),
        updated_at: row.get("updated_at"),
    })
}

/// Forgets the resume state once a transfer finished or was thrown away.
pub(crate) async fn clear_plugin_transfer(
    connection: &mut SqliteConnection,
    id: DownloadId,
) -> Result<()> {
    sqlx::query("DELETE FROM plugin_transfers WHERE download_id = ?")
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?;
    Ok(())
}

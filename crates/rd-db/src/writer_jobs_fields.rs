//! Per-download field writes of the writer: renames, the transform key reference and the
//! resolver refresh and pin claims.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind};
use sqlx::{Connection, Row};

use crate::{error::StoreError, writer::Writer};

impl Writer {
    /// User-driven rename; refused while the file is active or already finished.
    pub(crate) async fn rename_download(
        &mut self,
        id: DownloadId,
        file_name: &str,
    ) -> Result<DownloadFile> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if !matches!(
            current.state,
            DownloadState::Queued
                | DownloadState::Paused
                | DownloadState::RetryWait
                | DownloadState::Failed
                | DownloadState::Blocked
                | DownloadState::Cancelled
        ) {
            bail!(StoreError::wrong_state(
                "download cannot be renamed while active or completed"
            ));
        }
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "renamed": true }),
        );
        let mut transaction = self.connection.begin().await?;
        // The PAR2 marking follows the name it was taken on (RD-108-23).
        sqlx::query(
            "UPDATE downloads SET file_name = ?, recovery = ?, updated_at = ? WHERE id = ?",
        )
        .bind(file_name)
        .bind(rd_core::is_recovery_volume(file_name))
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        crate::writer::insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))
    }

    pub(crate) async fn set_file_name(&mut self, id: DownloadId, file_name: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE downloads SET file_name = ?, recovery = ?, updated_at = ? WHERE id = ?",
        )
        .bind(file_name)
        .bind(rd_core::is_recovery_volume(file_name))
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut self.connection)
        .await?;
        if result.rows_affected() != 1 {
            bail!(StoreError::not_found("download not found"));
        }
        Ok(())
    }

    /// Records which vault entry holds this download's transform key (RD-120-11).
    ///
    /// Written once per key rather than per attempt: the reference is part of
    /// `ContentTransform::fingerprint`, so a new one on every attempt would tell every
    /// continuation that its own chunk MACs belonged to somebody else.
    pub(crate) async fn set_transform_key_ref(
        &mut self,
        id: DownloadId,
        reference: Option<String>,
    ) -> Result<()> {
        let result =
            sqlx::query("UPDATE downloads SET transform_key_ref = ?, updated_at = ? WHERE id = ?")
                .bind(reference)
                .bind(Utc::now())
                .bind(id.to_string())
                .execute(&mut self.connection)
                .await?;
        if result.rows_affected() != 1 {
            bail!(StoreError::not_found("download not found"));
        }
        Ok(())
    }

    pub(crate) async fn claim_resolver_refresh(&mut self, id: DownloadId) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE downloads SET resolver_refresh_count = 1, updated_at = ? \
             WHERE id = ? AND resolver_refresh_count = 0",
        )
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut self.connection)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn claim_resolver_pin(
        &mut self,
        id: DownloadId,
        pin: rd_core::ResolverPin,
    ) -> Result<rd_core::ResolverPin> {
        sqlx::query(
            "INSERT INTO download_resolver_pins \
             (download_id, plugin_id, plugin_version, created_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(download_id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(pin.plugin_id.to_string())
        .bind(&pin.version)
        .bind(Utc::now())
        .execute(&mut self.connection)
        .await?;
        let row = sqlx::query(
            "SELECT plugin_id, plugin_version FROM download_resolver_pins WHERE download_id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&mut self.connection)
        .await?
        .context("download resolver pin was not persisted")?;
        Ok(rd_core::ResolverPin {
            plugin_id: crate::parse_id(row.get::<String, _>("plugin_id").as_str())?,
            version: row.get("plugin_version"),
        })
    }
}

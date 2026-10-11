//! The download window of a package and of a category (RD-1240-30). See
//! `migrations/0139_download_window.sql`.
//!
//! One statement per change, so there is no state between two writes for a crash to leave; the
//! event that tells the interface to read the package or category again is written in the same
//! transaction.

use anyhow::Result;
use chrono::Utc;
use rd_core::{CategoryId, DownloadWindow, EventEnvelope, EventKind, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::{
    Database,
    commands::{ConfigCommand, PackagesCommand},
    writer,
};

impl Database {
    /// Sets the package's own download window, or removes it with `None` so the package
    /// follows its category's; `false` when there is no such package.
    pub async fn set_package_download_window(
        &self,
        id: PackageId,
        window: Option<DownloadWindow>,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            PackagesCommand::SetPackageDownloadWindow { id, window, reply }
        })
        .await
    }

    /// Sets the download window of a category's packages, or removes it with `None`; `false`
    /// when there is no such category.
    pub async fn set_category_download_window(
        &self,
        id: CategoryId,
        window: Option<DownloadWindow>,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            ConfigCommand::SetCategoryDownloadWindow { id, window, reply }
        })
        .await
    }
}

/// The stored spelling of a window: JSON, or NULL for none.
pub(crate) fn window_json(window: Option<&DownloadWindow>) -> Result<Option<String>> {
    window
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

/// The writer half for a package: one update and its `package.state` event.
pub(crate) async fn set_package_download_window(
    connection: &mut SqliteConnection,
    id: PackageId,
    window: Option<DownloadWindow>,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let changed =
        sqlx::query("UPDATE packages SET download_window_json = ?, updated_at = ? WHERE id = ?")
            .bind(window_json(window.as_ref())?)
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?
            .rows_affected()
            > 0;
    let event = EventEnvelope::new(
        EventKind::PackageState,
        serde_json::json!({ "updated_packages": u8::from(changed) }),
    );
    writer::insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((changed, event))
}

/// The writer half for a category: one update and its `category.changed` event.
pub(crate) async fn set_category_download_window(
    connection: &mut SqliteConnection,
    id: CategoryId,
    window: Option<DownloadWindow>,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let changed =
        sqlx::query("UPDATE categories SET download_window_json = ?, updated_at = ? WHERE id = ?")
            .bind(window_json(window.as_ref())?)
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?
            .rows_affected()
            > 0;
    let event = EventEnvelope::new(
        EventKind::CategoryChanged,
        serde_json::json!({ "resource": "category", "id": id }),
    );
    writer::insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((changed, event))
}

#[cfg(test)]
#[path = "download_window_store_tests.rs"]
mod tests;

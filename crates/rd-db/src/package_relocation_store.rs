//! Pointing a package at the folder a torrent move carried its files to (RD-1100-10).
//!
//! The commit step of `rd_torrent::relocate`: the files are already at the new place, the
//! originals not yet released. Unlike a category change or a folder rename it records no
//! `previous_destination` — the torrent move owns its files and its own journal, and the
//! scheduler's sweep must not run a second move over the same folder.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::writer::insert_event;

/// Points the package at `to`, but only while it still names `from`.
///
/// One transaction with the rewrite of every absolute path stored for the package, as the folder
/// rename does. Returns `false` and changes nothing when the package no longer names `from` —
/// already switched by an earlier attempt, changed by somebody else, or gone — so a repeated
/// commit is harmless and the caller decides from the package what the move's outcome is.
pub(crate) async fn switch_package_destination(
    connection: &mut SqliteConnection,
    id: PackageId,
    from: &str,
    to: &str,
) -> Result<(bool, EventEnvelope)> {
    let now = Utc::now();
    let package = id.to_string();
    let mut transaction = connection.begin().await?;
    let switched = sqlx::query(
        "UPDATE packages SET destination = ?, updated_at = ? WHERE id = ? AND destination = ?",
    )
    .bind(to)
    .bind(now)
    .bind(&package)
    .bind(from)
    .execute(&mut *transaction)
    .await?
    .rows_affected()
        == 1;
    let event = EventEnvelope::new(
        EventKind::PackageState,
        serde_json::json!({ "relocated_packages": u8::from(switched) }),
    );
    if !switched {
        transaction.rollback().await?;
        return Ok((false, event));
    }
    let import_id =
        sqlx::query_scalar::<_, Option<String>>("SELECT nzb_import_id FROM packages WHERE id = ?")
            .bind(&package)
            .fetch_one(&mut *transaction)
            .await?;
    if !from.is_empty() {
        crate::package_store::rewrite_stored_paths(
            &mut transaction,
            &package,
            import_id.as_deref(),
            from,
            to,
            now,
        )
        .await?;
    }
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((true, event))
}

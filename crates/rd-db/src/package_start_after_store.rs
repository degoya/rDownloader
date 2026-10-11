//! A package's "not before" moment (RD-1240-14). See `migrations/0138_package_start_after.sql`.
//!
//! One statement per change, so there is no state between two writes for a crash to leave; the
//! event that tells the interface to read the package again is written in the same transaction.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, EventKind, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::{Database, commands::PackagesCommand, writer};

impl Database {
    /// Sets the moment the package's files may start from, or removes it with `None`; `false`
    /// when there is no such package.
    pub async fn set_package_start_after(
        &self,
        id: PackageId,
        start_after: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            PackagesCommand::SetPackageStartAfter {
                id,
                start_after,
                reply,
            }
        })
        .await
    }
}

/// The writer half: one update and its `package.state` event.
pub(crate) async fn set_package_start_after(
    connection: &mut SqliteConnection,
    id: PackageId,
    start_after: Option<DateTime<Utc>>,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let changed = sqlx::query("UPDATE packages SET start_after = ?, updated_at = ? WHERE id = ?")
        .bind(start_after)
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

#[cfg(test)]
mod tests {
    use chrono::{Duration, SubsecRound, Utc};
    use rd_core::{DownloadPriority, PackageId};

    use crate::{Database, NewPackage};

    #[tokio::test]
    async fn the_moment_survives_a_reopen_and_can_be_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("start-after.sqlite3");
        let database = Database::open(&path).await.expect("database");
        let package = NewPackage {
            id: PackageId::new(),
            name: "Tonight".to_owned(),
            destination: directory
                .path()
                .join("Tonight")
                .to_string_lossy()
                .into_owned(),
            category_id: None,
            priority: DownloadPriority::default(),
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        };
        let id = package.id;
        let created = database.create_package(package).await.expect("package");
        assert_eq!(created.start_after, None);

        let at = (Utc::now() + Duration::hours(3)).trunc_subsecs(0);
        assert!(
            database
                .set_package_start_after(id, Some(at))
                .await
                .expect("set")
        );
        database.close().await.expect("close");

        let database = Database::open(&path).await.expect("reopen");
        let read = database
            .get_package(id)
            .await
            .expect("read")
            .expect("there");
        assert_eq!(read.start_after, Some(at));

        assert!(
            database
                .set_package_start_after(id, None)
                .await
                .expect("clear")
        );
        let read = database
            .get_package(id)
            .await
            .expect("read")
            .expect("there");
        assert_eq!(read.start_after, None);
        assert!(
            !database
                .set_package_start_after(PackageId::new(), Some(at))
                .await
                .expect("missing")
        );
    }
}

//! A package's own download limit (RD-1100-01). See `migrations/0121_package_speed_limits.sql`.
//!
//! One statement per change, so there is no state between two writes for a crash to leave.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::PackageId;
use sqlx::{SqliteConnection, SqlitePool};

use crate::{Database, commands::PackagesCommand, parse_id, timestamp, writer};

impl Database {
    /// The package's own download limit in bytes per second; `None` when it has none.
    pub async fn package_speed_limit(&self, package_id: PackageId) -> Result<Option<u64>> {
        let rate: Option<i64> = sqlx::query_scalar(
            "SELECT download_bytes_per_second FROM package_speed_limits WHERE package_id = ?",
        )
        .bind(package_id.to_string())
        .fetch_optional(&self.readers)
        .await?;
        rate.map(stored_rate).transpose()
    }

    /// Every package that has a limit of its own, for the limiter registry.
    pub async fn package_speed_limits(&self) -> Result<Vec<(PackageId, u64)>> {
        list(&self.readers).await
    }

    /// Sets the package's own download limit, or removes it with `None`.
    pub async fn set_package_speed_limit(
        &self,
        id: PackageId,
        bytes_per_second: Option<u64>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            PackagesCommand::SetPackageSpeedLimit {
                id,
                bytes_per_second,
                reply,
            }
        })
        .await
    }
}

async fn list(pool: &SqlitePool) -> Result<Vec<(PackageId, u64)>> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT package_id, download_bytes_per_second FROM package_speed_limits")
            .fetch_all(pool)
            .await?;
    rows.into_iter()
        .map(|(id, rate)| Ok((parse_id(&id)?, stored_rate(rate)?)))
        .collect()
}

fn stored_rate(value: i64) -> Result<u64> {
    u64::try_from(value).context("negative package speed limit")
}

/// The writer half: one upsert, or one delete for `None` (and for zero, which is no limit).
pub(crate) async fn set_package_speed_limit(
    connection: &mut SqliteConnection,
    id: PackageId,
    bytes_per_second: Option<u64>,
) -> Result<()> {
    match bytes_per_second.filter(|rate| *rate > 0) {
        None => {
            sqlx::query("DELETE FROM package_speed_limits WHERE package_id = ?")
                .bind(id.to_string())
                .execute(connection)
                .await?;
        }
        Some(rate) => {
            sqlx::query(
                "INSERT INTO package_speed_limits (package_id, download_bytes_per_second, updated_at) \
                 VALUES (?, ?, ?) \
                 ON CONFLICT(package_id) DO UPDATE SET \
                   download_bytes_per_second = excluded.download_bytes_per_second, \
                   updated_at = excluded.updated_at",
            )
            .bind(id.to_string())
            .bind(i64::try_from(rate).context("package speed limit out of range")?)
            .bind(timestamp(&Utc::now()))
            .execute(connection)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rd_core::{DownloadPriority, PackageId};
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    use crate::{Database, NewPackage};

    async fn package(database: &Database, directory: &std::path::Path) -> PackageId {
        let package = NewPackage {
            id: PackageId::new(),
            name: "Limited".to_owned(),
            destination: directory.join("Limited").to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::default(),
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        };
        let id = package.id;
        database.create_package(package).await.expect("package");
        id
    }

    #[tokio::test]
    async fn a_limit_survives_a_reopen_and_goes_with_its_package() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("limits.sqlite3");
        let database = Database::open(&path).await.expect("database");
        let limited = package(&database, directory.path()).await;
        let other = package(&database, directory.path()).await;
        database
            .set_package_speed_limit(limited, Some(250_000))
            .await
            .expect("set");
        database
            .set_package_speed_limit(limited, Some(125_000))
            .await
            .expect("change");
        database.close().await.expect("close");

        let database = Database::open(&path).await.expect("reopen");
        assert_eq!(
            database.package_speed_limit(limited).await.expect("read"),
            Some(125_000)
        );
        assert_eq!(
            database.package_speed_limit(other).await.expect("read"),
            None
        );
        assert_eq!(
            database.package_speed_limits().await.expect("list"),
            vec![(limited, 125_000)]
        );

        // Zero is no limit, the same as clearing it.
        database
            .set_package_speed_limit(limited, Some(0))
            .await
            .expect("zero");
        assert!(
            database
                .package_speed_limits()
                .await
                .expect("list")
                .is_empty()
        );

        database
            .set_package_speed_limit(other, Some(1_000))
            .await
            .expect("set");
        database.close().await.expect("close");

        // The queue removes a package's row once its last file is gone; the limit goes with it.
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("connection");
        sqlx::query("DELETE FROM packages WHERE id = ?")
            .bind(other.to_string())
            .execute(&mut connection)
            .await
            .expect("delete package");
        connection.close().await.expect("close connection");
        let database = Database::open(&path).await.expect("reopen");
        assert!(
            database
                .package_speed_limits()
                .await
                .expect("list")
                .is_empty()
        );
    }
}

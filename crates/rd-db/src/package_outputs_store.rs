//! What a package demonstrably put on disk besides its downloads (RD-1390-07): the entries an
//! unpack created in its folder, and the files its Usenet downloads were assembled into.
//!
//! A full package reset reads this before it deletes anything, so it deletes exactly what the
//! package produced and never an entry that was in a shared folder before, which may belong to
//! somebody else.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{PackageId, PostprocessKind};
use sqlx::{Connection, SqliteConnection, SqlitePool};

/// What a full package reset may delete beyond its downloads' own files, read before it starts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageResetPlan {
    /// The entries an unpack created in the package's folder, absolute: a file, or a folder that
    /// did not exist before and holds nothing but the archive's content.
    pub unpack_outputs: Vec<String>,
    /// The folders unpacks wrote into (their steps' output path), removed once they are empty.
    pub unpack_folders: Vec<String>,
    /// The final files of the package's Usenet downloads, as their assembly recorded them.
    pub usenet_outputs: Vec<String>,
}

/// Adds `paths` to what the package produced; a path already recorded stays as it was.
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    package_id: PackageId,
    paths: &[String],
    at: DateTime<Utc>,
) -> Result<()> {
    let mut transaction = connection.begin().await?;
    for path in paths {
        sqlx::query(
            "INSERT OR IGNORE INTO package_outputs (package_id, path, recorded_at) \
             VALUES (?, ?, ?)",
        )
        .bind(package_id.to_string())
        .bind(path)
        .bind(at)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Forgets what the package produced, inside the caller's transaction: the reset deleted it.
pub(crate) async fn forget(connection: &mut SqliteConnection, package_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM package_outputs WHERE package_id = ?")
        .bind(package_id)
        .execute(connection)
        .await?;
    Ok(())
}

/// What a full reset of `package_id` may delete beyond its downloads' own files.
pub(crate) async fn plan(pool: &SqlitePool, package_id: PackageId) -> Result<PackageResetPlan> {
    let owner = package_id.to_string();
    let unpack_outputs = sqlx::query_scalar::<_, String>(
        "SELECT path FROM package_outputs WHERE package_id = ? ORDER BY path",
    )
    .bind(&owner)
    .fetch_all(pool)
    .await?;
    let mut unpack_folders = Vec::new();
    for kind in [
        PostprocessKind::ExtractZip,
        PostprocessKind::ExtractSevenZip,
        PostprocessKind::ExtractRar,
    ] {
        unpack_folders.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT output_path FROM postprocess_steps \
                 WHERE owner_id = ? AND kind = ? AND output_path IS NOT NULL",
            )
            .bind(&owner)
            .bind(crate::enum_string(kind)?)
            .fetch_all(pool)
            .await?,
        );
    }
    unpack_folders.sort();
    unpack_folders.dedup();
    let usenet_outputs = sqlx::query_scalar::<_, String>(
        "SELECT nzb_files.output_path FROM nzb_files \
         JOIN downloads ON downloads.nzb_file_id = nzb_files.id \
         WHERE downloads.package_id = ? AND nzb_files.output_path IS NOT NULL",
    )
    .bind(&owner)
    .fetch_all(pool)
    .await?;
    Ok(PackageResetPlan {
        unpack_outputs,
        unpack_folders,
        usenet_outputs,
    })
}

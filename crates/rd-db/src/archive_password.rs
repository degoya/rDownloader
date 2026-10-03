//! Archive passwords in the vault, the SQL half (RD-190-04).
//!
//! Four tables hold an archive password: `packages`, `collector_packages`, `nzb_imports` and
//! `subscription_items`. Each row keeps a `password_ref` into `rd-secrets`, never the value;
//! the old `password` column is only read once more, by the takeover of a start, and emptied
//! there. Every row owns its own vault entry: a password handed from the LinkGrabber to the
//! queue, from an NZB import to its package or from a subscription to the LinkGrabber is
//! written again under a reference of its own, so deleting one row can never take another
//! row's password with it.
//!
//! A write runs in three steps, and the order is what makes a stop anywhere harmless:
//!
//! 1. [`reserve`]: the new references go into `archive_password_sweep` with `reserved = 1`.
//! 2. The vault writes each value under its reference (`SecretStore::put_at`).
//! 3. [`adopt`]: one transaction points each row at its reference, empties the plain column
//!    and drops the reservation. A row that is gone by then turns its reservation into a
//!    release instead.
//!
//! A stop after step 1 or 2 leaves reservations, and only a start removes those — the vault
//! entries with them — because only at a start is no write under way. Whatever a row lets go
//! of (a delete, a cascade, a replaced reference) the triggers of migration `0113` record as
//! released in the same transaction, and [`swept`] / [`forget`] are the two halves of the
//! sweep that removes it from the vault.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{Connection, SqliteConnection, SqlitePool};

/// A table whose rows carry an archive password.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasswordTable {
    Packages,
    CollectorPackages,
    NzbImports,
    SubscriptionItems,
}

impl PasswordTable {
    /// Every table, in the order the takeover walks them.
    pub(crate) const ALL: [Self; 4] = [
        Self::Packages,
        Self::CollectorPackages,
        Self::NzbImports,
        Self::SubscriptionItems,
    ];

    /// The table's name. A fixed list: no statement here ever names a table a caller chose.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Packages => "packages",
            Self::CollectorPackages => "collector_packages",
            Self::NzbImports => "nzb_imports",
            Self::SubscriptionItems => "subscription_items",
        }
    }
}

/// The reference each of `ids` holds, for the rows that hold one.
pub(crate) async fn references(
    pool: &SqlitePool,
    table: PasswordTable,
    ids: &[String],
) -> Result<HashMap<String, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(format!(
        "SELECT id, password_ref FROM {} WHERE password_ref IS NOT NULL \
         AND id IN (SELECT value FROM json_each(?))",
        table.name()
    )))
    .bind(serde_json::to_string(ids)?)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Every reference the table's rows hold, by row id: what a full backup seals.
pub(crate) async fn all_references(
    pool: &SqlitePool,
    table: PasswordTable,
) -> Result<Vec<(String, String)>> {
    Ok(
        sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(format!(
            "SELECT id, password_ref FROM {} WHERE password_ref IS NOT NULL ORDER BY id",
            table.name()
        )))
        .fetch_all(pool)
        .await?,
    )
}

/// The rows that still hold a value in the plain column, with it: what a start takes over.
pub(crate) async fn plain(
    pool: &SqlitePool,
    table: PasswordTable,
) -> Result<Vec<(String, String)>> {
    Ok(
        sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(format!(
            "SELECT id, password FROM {} WHERE password IS NOT NULL",
            table.name()
        )))
        .fetch_all(pool)
        .await?,
    )
}

/// Step 1: records the references a write is about to fill.
pub(crate) async fn reserve(
    connection: &mut SqliteConnection,
    references: &[String],
) -> Result<()> {
    let mut transaction = connection.begin().await?;
    for reference in references {
        sqlx::query("INSERT INTO archive_password_sweep (reference, reserved) VALUES (?, 1)")
            .bind(reference)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Hands reservations whose write failed to the sweep.
pub(crate) async fn release(
    connection: &mut SqliteConnection,
    references: &[String],
) -> Result<()> {
    let mut transaction = connection.begin().await?;
    for reference in references {
        sqlx::query("UPDATE archive_password_sweep SET reserved = 0 WHERE reference = ?")
            .bind(reference)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Step 3: points each row at its reference — `None` clears the password — and empties the
/// plain column, in one transaction. The reference a row held before is released by the
/// trigger on `password_ref`.
pub(crate) async fn adopt(
    connection: &mut SqliteConnection,
    table: PasswordTable,
    entries: &[(String, Option<String>)],
) -> Result<()> {
    let statement = format!(
        "UPDATE {} SET password_ref = ?, password = NULL WHERE id = ?",
        table.name()
    );
    let mut transaction = connection.begin().await?;
    for (id, reference) in entries {
        let updated = sqlx::query(sqlx::AssertSqlSafe(&*statement))
            .bind(reference)
            .bind(id)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        let Some(reference) = reference else { continue };
        // A row deleted while its value was written leaves the entry to nobody.
        let settle = if updated == 0 {
            "UPDATE archive_password_sweep SET reserved = 0 WHERE reference = ?"
        } else {
            "DELETE FROM archive_password_sweep WHERE reference = ?"
        };
        sqlx::query(settle)
            .bind(reference)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// The references the sweep removes from the vault: the released ones, and at a start the
/// reservations too.
pub(crate) async fn swept(pool: &SqlitePool, with_reservations: bool) -> Result<Vec<String>> {
    let statement = if with_reservations {
        "SELECT reference FROM archive_password_sweep ORDER BY reference"
    } else {
        "SELECT reference FROM archive_password_sweep WHERE reserved = 0 ORDER BY reference"
    };
    Ok(sqlx::query_scalar::<_, String>(statement)
        .fetch_all(pool)
        .await?)
}

/// Drops the sweep rows whose vault entries are gone.
pub(crate) async fn forget(connection: &mut SqliteConnection, references: &[String]) -> Result<()> {
    let mut transaction = connection.begin().await?;
    for reference in references {
        sqlx::query("DELETE FROM archive_password_sweep WHERE reference = ?")
            .bind(reference)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

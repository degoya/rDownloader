//! The vault's sweep at start (DB-03): every entry no cell of the database names any more.
//!
//! Every owner but the archive passwords writes its value before its row and removes it after
//! the row is gone. A stop between the two, or a reference that could not be read before a
//! delete, leaves an entry that stays decryptable for ever with nothing pointing at it. The
//! sweep is the one place those are found: the vault's own listing minus every reference the
//! database holds.
//!
//! "Every reference the database holds" is read from every text and blob cell of every table,
//! whatever the column's declared type, not from a list of `*_ref` columns. The lists in [`crate::restore_copy`] name the columns, but references
//! also live inside JSON documents — the sign-in provider's client secret and the captcha
//! solver's key in `settings` — and a column added later without its list entry would cost a
//! credential, which is the one mistake a sweep must not make. Blobs count too (RA-DB-06): a
//! checkpoint or a remote job's source is opaque to the database and could carry one, and
//! SQLite keeps a text written into a column of any declared type as text. Keeping an orphan a
//! little longer is the safe direction. Table and column names come from the database's own schema, quoted,
//! never from a request.
//!
//! It runs once, at start, before anything that writes the vault is running: a request that
//! wrote its value and has not yet written its row would otherwise lose the value.

use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::{Connection, Row, SqliteConnection};

use crate::Database;

impl Database {
    /// Removes every vault entry no cell of the database names (DB-03), and answers how many
    /// went. Without an installed vault there is nothing to sweep.
    ///
    /// For the start only, before the scheduler, the plugins or the HTTP surface run.
    ///
    /// # Errors
    ///
    /// When the vault's folder or the database cannot be read; nothing is removed then. An
    /// entry that cannot be removed is logged and left for the next start.
    pub async fn sweep_vault(&self) -> Result<usize> {
        let Some(vault) = self.secret_vault() else {
            return Ok(0);
        };
        // The listing first: an entry written after it is not considered, whatever the
        // database says about it.
        let stored = vault.stored_references().await?;
        if stored.is_empty() {
            return Ok(0);
        }
        let mut connection = self.readers.acquire().await?;
        let named = named_references(&mut connection).await?;
        drop(connection);
        let mut removed = 0;
        for reference in stored
            .into_iter()
            .filter(|reference| !named.contains(reference))
        {
            match vault.remove(&reference).await {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::warn!(%error, "an orphaned vault entry stays until the next start");
                }
            }
            rd_core::failpoint!("vault.after_orphan_removed", || {
                anyhow::anyhow!("crash point: one orphaned vault entry is removed, others remain")
            });
        }
        Ok(removed)
    }
}

/// Every vault reference any text or blob cell of the database holds, read in one transaction.
async fn named_references(connection: &mut SqliteConnection) -> Result<HashSet<String>> {
    let mut tx = connection.begin().await?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )
    .fetch_all(&mut *tx)
    .await
    .context("list the tables")?;
    let mut named = HashSet::new();
    for table in tables {
        let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
            .bind(&table)
            .fetch_all(&mut *tx)
            .await
            .with_context(|| format!("list the columns of {table}"))?;
        let columns: Vec<String> = columns.iter().map(|name| quoted(name)).collect();
        if columns.is_empty() {
            continue;
        }
        // Compared and read as bytes, so a blob that is no UTF-8 cannot fail the sweep and a
        // text compares exactly as before.
        let holds = |column: &String| {
            format!(
                "(typeof({column}) IN ('text', 'blob') \
                 AND instr(CAST({column} AS BLOB), CAST('vault://' AS BLOB)) > 0)"
            )
        };
        let select = columns
            .iter()
            .map(|column| {
                format!(
                    "CASE WHEN {} THEN CAST({column} AS BLOB) END",
                    holds(column)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let filter = columns.iter().map(holds).collect::<Vec<_>>().join(" OR ");
        let statement = format!("SELECT {select} FROM {} WHERE {filter}", quoted(&table));
        let rows = sqlx::query(sqlx::AssertSqlSafe(statement))
            .fetch_all(&mut *tx)
            .await
            .with_context(|| format!("read the references of {table}"))?;
        for row in rows {
            for index in 0..columns.len() {
                if let Some(bytes) = row.try_get::<Option<Vec<u8>>, _>(index)? {
                    named.extend(rd_secrets::references_in(&String::from_utf8_lossy(&bytes)));
                }
            }
        }
    }
    tx.commit().await?;
    Ok(named)
}

/// An identifier from the schema as SQLite quotes it.
fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

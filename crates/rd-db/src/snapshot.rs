//! The consistent copy a full backup is built from, and what is read out of it (RD-160-01).
//!
//! `VACUUM INTO` runs on the writer's connection, as a writer command like the WAL checkpoint:
//! the copy is therefore exactly the state after every command sent before it and before any
//! command sent after it — a point in the one order every mutation of the process goes through.
//! It holds writes back for as long as the copy takes, which for this database is seconds;
//! downloads keep running and their checkpoints simply wait their turn.
//!
//! The parts a backup carries next to the copy (the plugin trust, the partial transfers) are
//! read from the *copy*, never from the live database, so they describe the same instant.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

/// The tables that make up the plugin trust: the keys confirmed on first use, the withdrawn
/// digests, and the repositories with their withdrawn keys, installs and version choices
/// (RD-140-01, RD-140-02). The settings bundle carries none of them.
pub const PLUGIN_TRUST_TABLES: &[&str] = &[
    "plugin_trusted_keys",
    "plugin_digest_revocations",
    "plugin_repositories",
    "plugin_withdrawn_keys",
    "plugin_repository_installs",
    "plugin_version_choices",
];

/// Opens a snapshot file read-only. It is never the live database.
async fn open(snapshot: &Path) -> Result<SqliteConnection> {
    let options = SqliteConnectOptions::new()
        .filename(snapshot)
        .read_only(true)
        .create_if_missing(false);
    SqliteConnection::connect_with(&options)
        .await
        .with_context(|| format!("open database snapshot {}", snapshot.display()))
}

/// Every row of each named table, as JSON objects by column name, in insertion order.
///
/// The column list comes from SQLite itself, so a column a later migration adds is carried
/// without this function changing. Only names from a fixed list reach the statement.
pub async fn read_tables(
    snapshot: &Path,
    tables: &[&'static str],
) -> Result<BTreeMap<String, Vec<serde_json::Value>>> {
    let mut connection = open(snapshot).await?;
    let mut dump = BTreeMap::new();
    for table in tables {
        let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
            .bind(*table)
            .fetch_all(&mut connection)
            .await
            .with_context(|| format!("read the columns of {table}"))?;
        anyhow::ensure!(!columns.is_empty(), "the snapshot has no table {table}");
        let object = columns
            .iter()
            .map(|column| {
                let quoted = column.replace('"', "\"\"");
                let literal = column.replace('\'', "''");
                format!("'{literal}', \"{quoted}\"")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT json_object({object}) FROM \"{table}\" ORDER BY rowid"
        )))
        .fetch_all(&mut connection)
        .await
        .with_context(|| format!("read {table}"))?;
        let rows = rows
            .iter()
            .map(|row| serde_json::from_str(row))
            .collect::<Result<Vec<serde_json::Value>, _>>()
            .with_context(|| format!("parse a row of {table}"))?;
        dump.insert((*table).to_owned(), rows);
    }
    connection.close().await.ok();
    Ok(dump)
}

/// Every download that has not finished, with what its resume depends on: the package folder
/// its part file lives in, the confirmed bytes and each chunk's checkpoint.
///
/// The checkpoints themselves are in the snapshot; this is the list a restore checks the part
/// files on disk against (RD-160-03) without opening the database first.
pub async fn read_partial_transfers(snapshot: &Path) -> Result<Vec<serde_json::Value>> {
    let mut connection = open(snapshot).await?;
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT json_object(\
            'id', d.id, 'package_id', d.package_id, 'kind', d.kind, \
            'file_name', d.file_name, 'state', d.state, 'destination', p.destination, \
            'total_bytes', d.total_bytes, 'committed_bytes', d.committed_bytes, \
            'chunks', json((SELECT json_group_array(json_object(\
                'start', c.start_offset, 'end', c.end_offset, 'committed', c.committed_offset)) \
                FROM chunks c WHERE c.download_id = d.id))) \
         FROM downloads d JOIN packages p ON p.id = d.package_id \
         WHERE d.state NOT IN ('completed', 'cancelled') \
         ORDER BY d.created_at, d.id",
    )
    .fetch_all(&mut connection)
    .await
    .context("read the unfinished downloads of the snapshot")?;
    connection.close().await.ok();
    rows.iter()
        .map(|row| serde_json::from_str(row).context("parse an unfinished download"))
        .collect()
}

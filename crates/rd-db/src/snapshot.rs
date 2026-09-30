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

/// Checks that a copy is whole with SQLite's own `PRAGMA integrity_check`: every page, every
/// index against its table, every constraint. Read-only. The copy before an update is not
/// published under its name until this passed (RD-180-03).
///
/// # Errors
///
/// When the copy does not open, or the check reports anything but `ok`; the first findings
/// are in the message.
pub async fn check_integrity(snapshot: &Path) -> Result<()> {
    let mut connection = open(snapshot).await?;
    let findings: Result<Vec<String>> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut connection)
        .await
        .with_context(|| format!("check the integrity of {}", snapshot.display()));
    connection.close().await.ok();
    let findings = findings?;
    if matches!(findings.as_slice(), [only] if only == "ok") {
        return Ok(());
    }
    anyhow::bail!(
        "{} is damaged: {}",
        snapshot.display(),
        findings.into_iter().take(5).collect::<Vec<_>>().join("; ")
    )
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

#[cfg(test)]
mod tests {
    use super::check_integrity;

    /// A copy `VACUUM INTO` wrote passes; the same file cut in half does not, whether SQLite
    /// refuses to open it or opens it and finds the damage.
    #[tokio::test]
    async fn a_whole_copy_passes_and_a_cut_one_does_not() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = crate::Database::open(directory.path().join("live.sqlite3"))
            .await
            .expect("database");
        let copy = directory.path().join("copy.sqlite3");
        database.snapshot_into(&copy).await.expect("snapshot");
        check_integrity(&copy).await.expect("a fresh copy is whole");

        let bytes = std::fs::read(&copy).expect("read copy");
        assert!(bytes.len() > 8192, "the copy is too small to cut");
        std::fs::write(&copy, &bytes[..bytes.len() / 2]).expect("cut copy");
        assert!(check_integrity(&copy).await.is_err());
    }
}

//! What a restored copy may bring along besides its rows (RD-1190-19).
//!
//! The copy becomes the live database at the next start, schema and all: a trigger or a view
//! somebody wrote into a crafted archive would run inside the service from then on. A copy that
//! passed its migrations must therefore hold exactly the objects this build's migrations create —
//! compared with a database migrated from nothing in memory. Tables are compared by name (a
//! column a migration added changes their stored text); every other object — index, trigger,
//! view — by its statement as well. SQLite's own objects (`sqlite_…`) are left out.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

/// `(type, name)` to the statement that created it, whitespace collapsed.
type Objects = BTreeMap<(String, String), String>;

async fn objects_of(connection: &mut SqliteConnection) -> Result<Objects> {
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT type, name, sql FROM sqlite_master")
            .fetch_all(&mut *connection)
            .await
            .context("read the schema")?;
    Ok(rows
        .into_iter()
        .filter(|(_, name, _)| !name.starts_with("sqlite_"))
        .map(|(kind, name, sql)| {
            let sql = sql
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            ((kind, name), sql)
        })
        .collect())
}

/// The objects a database migrated from nothing by this build holds.
async fn reference_objects() -> Result<Objects> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
        .await
        .context("open the reference schema")?;
    let objects = async {
        crate::MIGRATOR
            .run(&pool)
            .await
            .context("migrate the reference schema")?;
        let mut connection = pool.acquire().await.context("read the reference schema")?;
        objects_of(&mut connection).await
    }
    .await;
    pool.close().await;
    objects
}

/// Every schema object of a migrated copy that this build would not have created, or that it
/// lacks, as `type name` — empty for a copy whose schema is this build's own. Read-only.
///
/// # Errors
///
/// When the copy or the reference cannot be read.
pub async fn foreign_schema_objects(copy: &Path) -> Result<Vec<String>> {
    let mut connection = super::open_read_only(copy).await?;
    let found = objects_of(&mut connection).await;
    connection.close().await.ok();
    let found = found?;
    let reference = reference_objects().await?;
    let mut foreign = Vec::new();
    for ((kind, name), sql) in &found {
        match reference.get(&(kind.clone(), name.clone())) {
            None => foreign.push(format!("{kind} {name}")),
            Some(expected) if kind != "table" && expected != sql => {
                foreign.push(format!("{kind} {name}"));
            }
            Some(_) => {}
        }
    }
    for (kind, name) in reference.keys() {
        if !found.contains_key(&(kind.clone(), name.clone())) {
            foreign.push(format!("missing {kind} {name}"));
        }
    }
    Ok(foreign)
}

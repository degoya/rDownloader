//! The queue's stop mark (RD-1210-02). See `migrations/0133_queue_stop_mark.sql`.
//!
//! One statement per change, so there is no state between two writes for a crash to leave. The
//! mark goes with its file or package through the foreign keys; nothing here has to remember it.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{DownloadId, PackageId};
use sqlx::SqliteConnection;

use crate::{Database, commands::PackagesCommand, parse_id, parse_time, timestamp, writer};

/// What a stop mark sits on: one file, or one package as a whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopMarkTarget {
    Download(DownloadId),
    Package(PackageId),
}

/// The stop mark in force.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopMark {
    pub target: StopMarkTarget,
    pub set_at: DateTime<Utc>,
}

impl StopMarkTarget {
    /// The two columns, exactly one of them set.
    fn columns(self) -> (Option<String>, Option<String>) {
        match self {
            Self::Download(id) => (Some(id.to_string()), None),
            Self::Package(id) => (None, Some(id.to_string())),
        }
    }
}

impl Database {
    /// The stop mark in force, if any.
    pub async fn stop_mark(&self) -> Result<Option<StopMark>> {
        let row: Option<(Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT download_id, package_id, set_at FROM queue_stop_mark WHERE slot = 1",
        )
        .fetch_optional(&self.readers)
        .await?;
        row.map(|(download, package, set_at)| {
            let target = match (download, package) {
                (Some(id), None) => StopMarkTarget::Download(parse_id(&id)?),
                (None, Some(id)) => StopMarkTarget::Package(parse_id(&id)?),
                _ => bail!("a stop mark names neither or both of a file and a package"),
            };
            Ok(StopMark {
                target,
                set_at: parse_time(&set_at)?,
            })
        })
        .transpose()
    }

    /// Sets the stop mark on `target`, replacing the one in force.
    pub async fn set_stop_mark(&self, target: StopMarkTarget) -> Result<StopMark> {
        writer::request(&self.writer, |reply| PackagesCommand::SetStopMark {
            target,
            reply,
        })
        .await
    }

    /// Removes the stop mark; with `only`, only while it still sits on that target, so a mark
    /// set anew in the meantime is kept. Answers whether one was removed.
    pub async fn clear_stop_mark(&self, only: Option<StopMarkTarget>) -> Result<bool> {
        writer::request(&self.writer, |reply| PackagesCommand::ClearStopMark {
            only,
            reply,
        })
        .await
    }
}

/// The writer half of [`Database::set_stop_mark`]: one upsert of the single row.
pub(crate) async fn set_stop_mark(
    connection: &mut SqliteConnection,
    target: StopMarkTarget,
) -> Result<StopMark> {
    let (download, package) = target.columns();
    let set_at = Utc::now();
    sqlx::query(
        "INSERT INTO queue_stop_mark (slot, download_id, package_id, set_at) VALUES (1, ?, ?, ?) \
         ON CONFLICT(slot) DO UPDATE SET \
           download_id = excluded.download_id, \
           package_id = excluded.package_id, \
           set_at = excluded.set_at",
    )
    .bind(download)
    .bind(package)
    .bind(timestamp(&set_at))
    .execute(connection)
    .await?;
    Ok(StopMark {
        target,
        // What the row holds, read back the way `stop_mark` would read it.
        set_at: parse_time(&timestamp(&set_at))?,
    })
}

/// The writer half of [`Database::clear_stop_mark`]: one delete.
pub(crate) async fn clear_stop_mark(
    connection: &mut SqliteConnection,
    only: Option<StopMarkTarget>,
) -> Result<bool> {
    let result = match only {
        None => {
            sqlx::query("DELETE FROM queue_stop_mark")
                .execute(connection)
                .await?
        }
        Some(target) => {
            let (download, package) = target.columns();
            sqlx::query("DELETE FROM queue_stop_mark WHERE download_id IS ? AND package_id IS ?")
                .bind(download)
                .bind(package)
                .execute(connection)
                .await?
        }
    };
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
#[path = "stop_mark_store_tests.rs"]
mod tests;

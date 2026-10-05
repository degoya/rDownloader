//! The download history (RD-1100-04): one row per package that reached an outcome, kept after
//! the package left the queue.
//!
//! The entry is written by [`record`] inside the transaction that gives the package its
//! outcome, so the history never describes an outcome the queue does not know and never misses
//! one it does (crash point `history.before_entry_committed`). Nothing secret reaches it: the
//! sources go through [`rd_core::history_source`], the failure parameters were redacted before
//! they were stored on the download, and the archive password is never read.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{
    ByteCount, DownloadKind, HISTORY_MAX_SOURCES, HistoryEntry, HistoryOutcome, MessageParams,
};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};

/// One file of a package as the history keeps it: id, source, sizes, state, failure.
type FileRow = (String, String, Option<i64>, i64, String, Option<String>);

use crate::{enum_string, parse_enum, parse_time, timestamp};

/// Download states after which a file does nothing more by itself.
const SETTLED_FAILED: [&str; 4] = ["failed", "blocked", "cancelled", "skipped"];

/// The code of a package that failed in post-processing without a step that named one.
pub const POSTPROCESS_FAILED_CODE: &str = "history.postprocess_failed";
/// The code of a package whose unpack failed without a step that named one.
pub const UNPACK_FAILED_CODE: &str = "history.unpack_failed";
/// The code of a failed download whose failure carried no code of its own.
pub const DOWNLOAD_FAILED_CODE: &str = "history.download_failed";

/// What a list of the history asks for. Every filter is optional; the order is newest first.
#[derive(Clone, Debug, Default)]
pub struct HistoryQuery {
    /// Part of the name or of a source address, case-insensitive.
    pub search: Option<String>,
    pub outcome: Option<HistoryOutcome>,
    pub kind: Option<DownloadKind>,
    /// Entries that ended at or after this instant.
    pub finished_from: Option<DateTime<Utc>>,
    /// Entries that ended at or before this instant.
    pub finished_to: Option<DateTime<Utc>>,
    /// Leaves out the entries a SABnzbd client deleted from its history.
    pub compat_visible_only: bool,
    /// Rows to skip, then rows to return at most; `None` returns the rest.
    pub offset: u64,
    pub limit: Option<u64>,
}

/// One page of the history and how many entries the filters match in all.
#[derive(Clone, Debug, Default)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub total: u64,
}

/// Writes the entry of a package that just reached `outcome`, inside the caller's transaction.
///
/// An upsert keyed on the package: a package that is retried and ends again carries its latest
/// outcome, once. A package that is already gone writes nothing.
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    package_id: &str,
    outcome: HistoryOutcome,
    now: DateTime<Utc>,
) -> Result<()> {
    let Some(package) = sqlx::query(
        "SELECT packages.name, packages.kind, packages.destination, packages.created_at, \
         packages.extraction_result, packages.nzb_import_id, categories.name AS category \
         FROM packages LEFT JOIN categories ON categories.id = packages.category_id \
         WHERE packages.id = ?",
    )
    .bind(package_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(());
    };
    let files: Vec<FileRow> = sqlx::query_as(
        "SELECT id, source_url, total_bytes, committed_bytes, state, last_error_json \
         FROM downloads WHERE package_id = ? ORDER BY position ASC, created_at ASC",
    )
    .bind(package_id)
    .fetch_all(&mut *connection)
    .await?;

    let mut sources: Vec<String> = Vec::new();
    let mut total_bytes = 0u64;
    let mut file_count = 0u32;
    let mut download_failure: Option<rd_core::Failure> = None;
    for (id, source, total, committed, state, last_error) in &files {
        if let Ok(url) = url::Url::parse(source)
            && let Some(kept) = rd_core::history_source(&url)
            && sources.len() < HISTORY_MAX_SOURCES
            && !sources.contains(&kept)
        {
            sources.push(kept);
        }
        // A mirror that stood down moved nothing and is no file of the package.
        if state == "skipped" {
            continue;
        }
        file_count = file_count.saturating_add(1);
        let bytes = total.unwrap_or_default().max(*committed);
        total_bytes = total_bytes.saturating_add(u64::try_from(bytes).unwrap_or_default());
        if download_failure.is_none() && state == "failed" {
            download_failure = last_error.as_deref().and_then(|json| {
                crate::json_column::lenient(
                    serde_json::from_str(json),
                    "downloads",
                    "last_error_json",
                    id,
                )
            });
        }
    }

    let (error_code, error_params) = match outcome {
        HistoryOutcome::Completed => (None, MessageParams::new()),
        HistoryOutcome::Failed => {
            let import_id: Option<String> = package.try_get("nzb_import_id")?;
            let extraction: Option<String> = package.try_get("extraction_result")?;
            let (code, params) = failure_of(
                connection,
                package_id,
                import_id.as_deref(),
                extraction.as_deref(),
                download_failure,
            )
            .await?;
            (Some(code), params)
        }
    };

    let created_at: DateTime<Utc> = package.try_get("created_at")?;
    let name: String = package.try_get("name")?;
    let kind: String = package.try_get("kind")?;
    let destination: String = package.try_get("destination")?;
    let category: Option<String> = package.try_get("category")?;
    sqlx::query(
        "INSERT INTO download_history (package_id, name, kind, category, destination, \
         total_bytes, file_count, sources_json, outcome, error_code, error_params_json, \
         created_at, finished_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(package_id) DO UPDATE SET name = excluded.name, kind = excluded.kind, \
         category = excluded.category, destination = excluded.destination, \
         total_bytes = excluded.total_bytes, file_count = excluded.file_count, \
         sources_json = excluded.sources_json, outcome = excluded.outcome, \
         error_code = excluded.error_code, error_params_json = excluded.error_params_json, \
         finished_at = excluded.finished_at",
    )
    .bind(package_id)
    .bind(name)
    .bind(kind)
    .bind(category)
    .bind(destination)
    .bind(i64::try_from(total_bytes).unwrap_or(i64::MAX))
    .bind(i64::from(file_count))
    .bind(serde_json::to_string(&sources)?)
    .bind(enum_string(outcome)?)
    .bind(error_code)
    .bind(if error_params.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&error_params)?)
    })
    .bind(timestamp(&created_at))
    .bind(timestamp(&now))
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Writes a failed entry when the download that just failed was the package's last hope.
///
/// A package whose files all ended without a single one finishing never reaches
/// post-processing, so nothing gives it a package state of its own; without this its failure
/// would be lost the moment the package is removed. A package with a finished file is left to
/// post-processing, which records its outcome through [`record`].
pub(crate) async fn record_if_settled_failed(
    connection: &mut SqliteConnection,
    package_id: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM downloads WHERE package_id = ?")
            .bind(package_id)
            .fetch_all(&mut *connection)
            .await?;
    let settled = states
        .iter()
        .all(|state| SETTLED_FAILED.contains(&state.as_str()));
    if !settled || !states.iter().any(|state| state == "failed") {
        return Ok(());
    }
    record(connection, package_id, HistoryOutcome::Failed, now).await
}

/// The code and parameters a failed package is listed with, most specific first: the failed
/// post-processing step that named one, a failed unpack, a failed download, or the generic one.
async fn failure_of(
    connection: &mut SqliteConnection,
    package_id: &str,
    import_id: Option<&str>,
    extraction: Option<&str>,
    download_failure: Option<rd_core::Failure>,
) -> Result<(String, MessageParams)> {
    let step: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT code, params_json FROM postprocess_steps \
         WHERE owner_id IN (?, ?) AND state = 'failed' AND code IS NOT NULL \
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(package_id)
    .bind(import_id.unwrap_or(package_id))
    .fetch_optional(&mut *connection)
    .await?;
    if let Some((code, params)) = step {
        let params: MessageParams = params
            .as_deref()
            .and_then(|json| {
                crate::json_column::lenient(
                    serde_json::from_str(json),
                    "postprocess_steps",
                    "params_json",
                    format_args!("{package_id} {code}"),
                )
            })
            .unwrap_or_default();
        return Ok((code, params));
    }
    if extraction == Some("failed") {
        return Ok((UNPACK_FAILED_CODE.to_owned(), MessageParams::new()));
    }
    if let Some(failure) = download_failure {
        let failure = rd_core::redact_failure(failure);
        let code = failure
            .code
            .unwrap_or_else(|| DOWNLOAD_FAILED_CODE.to_owned());
        return Ok((code, failure.params));
    }
    Ok((POSTPROCESS_FAILED_CODE.to_owned(), MessageParams::new()))
}

const COLUMNS: &str = "id, package_id, name, kind, category, destination, total_bytes, \
     file_count, sources_json, outcome, error_code, error_params_json, created_at, finished_at";

/// `LIKE` treats `%` and `_` as wildcards; a person searching for `100%` means the characters.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn push_filters(builder: &mut QueryBuilder<Sqlite>, query: &HistoryQuery) -> Result<()> {
    builder.push(" WHERE 1 = 1");
    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pattern = format!("%{}%", escape_like(search));
        builder
            .push(" AND (name LIKE ")
            .push_bind(pattern.clone())
            .push(" ESCAPE '\\' OR sources_json LIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '\\')");
    }
    if let Some(outcome) = query.outcome {
        builder
            .push(" AND outcome = ")
            .push_bind(enum_string(outcome)?);
    }
    if let Some(kind) = query.kind {
        builder.push(" AND kind = ").push_bind(enum_string(kind)?);
    }
    if let Some(from) = query.finished_from {
        builder
            .push(" AND finished_at >= ")
            .push_bind(timestamp(&from));
    }
    if let Some(to) = query.finished_to {
        builder
            .push(" AND finished_at <= ")
            .push_bind(timestamp(&to));
    }
    if query.compat_visible_only {
        builder.push(" AND compat_hidden = 0");
    }
    Ok(())
}

/// The entries matching the query, newest first, and how many match in all.
pub(crate) async fn list(pool: &SqlitePool, query: &HistoryQuery) -> Result<HistoryPage> {
    let mut count: QueryBuilder<Sqlite> =
        QueryBuilder::new("SELECT COUNT(*) FROM download_history");
    push_filters(&mut count, query)?;
    let total: i64 = count.build_query_scalar::<i64>().fetch_one(pool).await?;

    let mut select: QueryBuilder<Sqlite> =
        QueryBuilder::new(format!("SELECT {COLUMNS} FROM download_history"));
    push_filters(&mut select, query)?;
    // `LIMIT -1` is SQLite's "no limit", which an `OFFSET` needs in front of it.
    select
        .push(" ORDER BY finished_at DESC, id DESC LIMIT ")
        .push_bind(
            query
                .limit
                .map_or(-1, |limit| i64::try_from(limit).unwrap_or(i64::MAX)),
        )
        .push(" OFFSET ")
        .push_bind(i64::try_from(query.offset).unwrap_or(i64::MAX));
    let rows = select.build().fetch_all(pool).await?;
    let entries = rows
        .iter()
        .map(entry_from_row)
        .collect::<Result<Vec<_>>>()?;
    Ok(HistoryPage {
        entries,
        total: u64::try_from(total).unwrap_or_default(),
    })
}

/// One entry by its id.
pub(crate) async fn get(pool: &SqlitePool, id: i64) -> Result<Option<HistoryEntry>> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM download_history WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .as_ref()
    .map(entry_from_row)
    .transpose()
}

fn entry_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<HistoryEntry> {
    let package_id: String = row.try_get("package_id")?;
    let kind: String = row.try_get("kind")?;
    let outcome: String = row.try_get("outcome")?;
    let total_bytes: i64 = row.try_get("total_bytes")?;
    let file_count: i64 = row.try_get("file_count")?;
    let sources: String = row.try_get("sources_json")?;
    let params: Option<String> = row.try_get("error_params_json")?;
    let created_at: String = row.try_get("created_at")?;
    let finished_at: String = row.try_get("finished_at")?;
    Ok(HistoryEntry {
        id: row.try_get("id")?,
        package_id: package_id.parse()?,
        name: row.try_get("name")?,
        kind: parse_enum(&kind)?,
        category: row.try_get("category")?,
        destination: row.try_get("destination")?,
        total_bytes: ByteCount::new(u64::try_from(total_bytes).unwrap_or_default())
            .map_err(anyhow::Error::msg)?,
        file_count: u32::try_from(file_count).unwrap_or(u32::MAX),
        sources: serde_json::from_str(&sources)?,
        outcome: parse_enum(&outcome)?,
        error_code: row.try_get("error_code")?,
        error_params: params
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default(),
        created_at: parse_time(&created_at)?,
        finished_at: parse_time(&finished_at)?,
    })
}

/// Removes what the retention no longer keeps: entries that ended before `older_than`, then
/// the oldest beyond `max_entries`. Returns how many went.
///
/// One statement each, not batches: the history holds at most the retention's cap of small
/// rows, and the sweep that calls this runs once a minute.
pub(crate) async fn prune(
    connection: &mut SqliteConnection,
    max_entries: u64,
    older_than: DateTime<Utc>,
) -> Result<u64> {
    let aged = sqlx::query("DELETE FROM download_history WHERE finished_at < ?")
        .bind(timestamp(&older_than))
        .execute(&mut *connection)
        .await?
        .rows_affected();
    let over = sqlx::query(
        "DELETE FROM download_history WHERE id IN (SELECT id FROM download_history \
         ORDER BY finished_at DESC, id DESC LIMIT -1 OFFSET ?)",
    )
    .bind(i64::try_from(max_entries).unwrap_or(i64::MAX))
    .execute(&mut *connection)
    .await?
    .rows_affected();
    Ok(aged + over)
}

/// Empties the history on request and reports how many entries went.
pub(crate) async fn clear(connection: &mut SqliteConnection) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM download_history")
        .execute(connection)
        .await?
        .rows_affected())
}

/// Hides the entries of these packages from the SABnzbd history, or every entry for `None`;
/// the native history keeps them.
pub(crate) async fn hide_from_compat(
    connection: &mut SqliteConnection,
    package_ids: Option<Vec<rd_core::PackageId>>,
) -> Result<u64> {
    let Some(ids) = package_ids else {
        return Ok(sqlx::query(
            "UPDATE download_history SET compat_hidden = 1 WHERE compat_hidden = 0",
        )
        .execute(connection)
        .await?
        .rows_affected());
    };
    let mut hidden = 0;
    for id in ids {
        hidden += sqlx::query("UPDATE download_history SET compat_hidden = 1 WHERE package_id = ?")
            .bind(id.to_string())
            .execute(&mut *connection)
            .await?
            .rows_affected();
    }
    Ok(hidden)
}

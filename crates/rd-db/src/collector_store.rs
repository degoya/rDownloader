use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rd_core::{
    CandidateId, CollectorBatch, EventEnvelope, EventKind, IngressSource, LinkCandidate,
};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, page_binds};

mod filter_decision;
mod intake;
mod rows;

pub(crate) use intake::{ADDRESS_TAKEN, add_batch};
use rows::BatchRow;
pub(crate) use rows::{CANDIDATE_SELECT, CandidateRow, GET_CANDIDATE};

/// The archive password each new package of a batch is to get, by package.
pub(crate) type BatchPasswords = Vec<(rd_core::CollectorPackageId, String)>;

/// One LinkGrabber submission.
pub struct NewCollectorBatch {
    pub source: IngressSource,
    pub source_label: Option<String>,
    /// Explicit package name (Click'n'Load `package` field or manual input).
    pub package_name: Option<String>,
    /// Archive password announced with the submission.
    pub password: Option<String>,
    /// Per-link archive passwords, parallel to `urls`.
    ///
    /// A subscription poll can submit several standalone releases at once. Keeping their
    /// passwords beside their links prevents the first release's password from being copied to
    /// every package in the batch. `password` remains the fallback for ordinary batch-wide
    /// intake such as a pasted list or DLC container.
    pub passwords: Vec<Option<String>>,
    /// Explicit category for every created package; `None` applies the normal routing rules.
    pub category_id: Option<rd_core::CategoryId>,
    /// Explicit package priority; `None` uses the normal priority.
    pub priority: Option<rd_core::DownloadPriority>,
    pub urls: Vec<Url>,
    /// Provider override per URL (parallel to `urls`); `None` = derive from the host.
    pub providers: Vec<Option<String>>,
    /// Optional file-name overrides parallel to `urls` (used by parsed local metadata).
    pub file_names: Vec<Option<String>>,
    /// Optional size overrides parallel to `urls`.
    pub sizes: Vec<Option<rd_core::ByteCount>>,
    /// Optional package suggestions parallel to `urls` — the folder a crawler read a link
    /// out of (RD-104-03). Links sharing one become one package; an explicit
    /// `package_name` still overrides all of them.
    pub package_hints: Vec<Option<String>>,
    /// What each link's source said about mirrors, parallel to `urls` (RD-110-18).
    ///
    /// The first of the three sources a mirror group can come from, and the only one that
    /// arrives from outside: a site rule whose page is one release states that its links are
    /// the same file. Stored as it came in and never recomputed, so the regroup that runs
    /// after the online check cannot lose it.
    pub mirror_hints: Vec<Option<rd_core::MirrorHint>>,
    /// Captured request metadata parallel to `urls` (intercepted browser downloads).
    pub requests: Vec<Option<rd_core::CapturedRequest>>,
    /// `vault://` reference of each captured request body, parallel to `urls`.
    ///
    /// Kept out of `requests` on purpose: the reference goes straight into its own column
    /// and never through a serializable struct.
    pub body_refs: Vec<Option<String>>,
    /// Start every fresh link in `checking` so the link check service probes it.
    pub auto_check: bool,
    /// What the source already knows about each link, parallel to `urls` (RD-107-02).
    ///
    /// Filled only by a subscription poll, with the attributes `attributes.rs` retained:
    /// `imdb`, `imdbscore`, `imdbplot`, `coverurl` and whatever else the indexer emitted,
    /// minus everything that gate discards. Reached later by an enricher, so it does not
    /// have to guess a title back out of a file name. An empty map means "nothing declared",
    /// which is every pasted, captured and container link.
    pub source_attributes: Vec<BTreeMap<String, String>>,
}

/// What the indexer declared about the hit behind one candidate (RD-107-02).
///
/// Empty for a link no subscription produced. The caller re-applies the `attributes.rs` gate
/// before anything leaves towards a plugin; this read is not itself that gate.
pub(crate) async fn source_attributes(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<BTreeMap<String, String>> {
    let stored = sqlx::query_scalar::<_, Option<String>>(
        "SELECT source_attributes_json FROM link_candidates WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(&mut *connection)
    .await?
    .flatten();
    Ok(stored
        .as_deref()
        .and_then(|value| {
            crate::json_column::lenient(
                serde_json::from_str(value),
                "link_candidates",
                "source_attributes_json",
                id,
            )
        })
        .unwrap_or_default())
}

/// Every batch, newest first; the id comes last so batches of the same instant still have one
/// order and a page cut from it in SQL is the same slice every time (RD-191-05).
const BATCH_LIST: &str = "SELECT id, source, source_label, created_at FROM collector_batches ORDER BY created_at DESC, id ASC";

pub(crate) async fn list_batches(pool: &SqlitePool) -> Result<Vec<CollectorBatch>> {
    sqlx::query_as::<_, BatchRow>(BATCH_LIST)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

/// One page of [`list_batches`] and how many batches there are, in one read transaction
/// (RD-191-05).
pub(crate) async fn batches_page(
    pool: &SqlitePool,
    offset: u64,
    limit: Option<u64>,
) -> Result<(Vec<CollectorBatch>, u64)> {
    let (limit, offset) = page_binds(offset, limit);
    let mut transaction = pool.begin().await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM collector_batches")
        .fetch_one(&mut *transaction)
        .await?;
    let rows = sqlx::query_as::<_, BatchRow>(sqlx::AssertSqlSafe(format!(
        "{BATCH_LIST} LIMIT ? OFFSET ?"
    )))
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let batches = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()?;
    Ok((batches, u64::try_from(total).unwrap_or_default()))
}

/// The links the LinkGrabber still shows: everything not handed to the queue yet.
const OPEN_CANDIDATES: &str = "WHERE state != 'enqueued'";

/// Package order, then the link's place in it; the id comes last for one fixed order
/// (RD-191-05).
const CANDIDATE_ORDER: &str = "ORDER BY \
     COALESCE((SELECT p.position FROM collector_packages p WHERE p.id = link_candidates.package_id), 0) ASC, \
     position ASC, created_at ASC, id ASC";

pub(crate) async fn list_candidates(pool: &SqlitePool) -> Result<Vec<LinkCandidate>> {
    sqlx::query_as::<_, CandidateRow>(sqlx::AssertSqlSafe(format!(
        "{CANDIDATE_SELECT} {OPEN_CANDIDATES} {CANDIDATE_ORDER}"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// One page of [`list_candidates`] and how many links that list holds, in one read transaction
/// (RD-191-05).
pub(crate) async fn candidates_page(
    pool: &SqlitePool,
    offset: u64,
    limit: Option<u64>,
) -> Result<(Vec<LinkCandidate>, u64)> {
    let (limit, offset) = page_binds(offset, limit);
    let mut transaction = pool.begin().await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM link_candidates {OPEN_CANDIDATES}"
    )))
    .fetch_one(&mut *transaction)
    .await?;
    let rows = sqlx::query_as::<_, CandidateRow>(sqlx::AssertSqlSafe(format!(
        "{CANDIDATE_SELECT} {OPEN_CANDIDATES} {CANDIDATE_ORDER} LIMIT ? OFFSET ?"
    )))
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let candidates = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()?;
    Ok((candidates, u64::try_from(total).unwrap_or_default()))
}

/// The `vault://` reference one candidate holds, if any (RD-110-38).
pub(crate) async fn secret_fragment_ref(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Option<String>> {
    use sqlx::Row;

    let row = sqlx::query("SELECT secret_fragment_ref FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|row| {
        row.try_get::<Option<String>, _>("secret_fragment_ref")
            .ok()
            .flatten()
    }))
}

/// Every vault reference the rows matching `predicate` hold, read before they are deleted:
/// the link fragment (RD-110-38) and the captured request body of a POST replay.
///
/// Read first and removed from the vault afterwards, because the reference is *in* the row:
/// once the row is gone there is nothing left to find the secret by, and it would sit in the
/// vault for the life of the installation. The predicate is the same one the delete uses.
async fn vault_refs_where(
    pool: &SqlitePool,
    predicate: &str,
    bind: Option<String>,
) -> Result<Vec<String>> {
    use sqlx::Row;

    let sql = format!(
        "SELECT secret_fragment_ref, replay_body_ref FROM link_candidates \
         WHERE (secret_fragment_ref IS NOT NULL OR replay_body_ref IS NOT NULL) AND {predicate}"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(&*sql));
    if let Some(value) = bind {
        query = query.bind(value);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .flat_map(|row| {
            ["secret_fragment_ref", "replay_body_ref"]
                .into_iter()
                .filter_map(|column| row.try_get::<Option<String>, _>(column).ok().flatten())
                .collect::<Vec<_>>()
        })
        .collect())
}

/// The vault references deleting one candidate is about to orphan.
pub(crate) async fn candidate_vault_refs(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Vec<String>> {
    vault_refs_where(pool, "id = ?", Some(id.to_string())).await
}

/// The references `delete_candidates` is about to orphan.
pub(crate) async fn deletable_vault_refs(pool: &SqlitePool) -> Result<Vec<String>> {
    vault_refs_where(pool, "state NOT IN ('resolving', 'enqueued')", None).await
}

/// The references deleting one LinkGrabber package is about to orphan.
pub(crate) async fn package_vault_refs(
    pool: &SqlitePool,
    id: rd_core::CollectorPackageId,
) -> Result<Vec<String>> {
    vault_refs_where(
        pool,
        "package_id = ? AND state != 'enqueued'",
        Some(id.to_string()),
    )
    .await
}

/// Clears a candidate's reference once a download row owns it.
pub(crate) async fn take_candidate_secret_fragment_ref(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: CandidateId,
) -> Result<()> {
    sqlx::query("UPDATE link_candidates SET secret_fragment_ref = NULL WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(crate) async fn delete_candidate(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<EventEnvelope> {
    let state = sqlx::query_scalar::<_, String>("SELECT state FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .context(StoreError::not_found("link candidate not found"))?;
    anyhow::ensure!(
        state != "resolving",
        StoreError::busy("link candidate is being processed")
    );
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id, "removed": true }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    delete_empty_batches(&mut transaction).await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

pub(crate) async fn delete_candidates(
    connection: &mut SqliteConnection,
) -> Result<(u64, EventEnvelope)> {
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "all_candidates_removed": true }),
    );
    let mut transaction = connection.begin().await?;
    let result =
        sqlx::query("DELETE FROM link_candidates WHERE state NOT IN ('resolving', 'enqueued')")
            .execute(&mut *transaction)
            .await?;
    delete_empty_batches(&mut transaction).await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected(), event))
}

pub(crate) async fn delete_empty_batches(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<()> {
    crate::collector_packages::delete_empty_packages(transaction).await?;
    // A batch still owning a package stays even without links of its own: a package made by
    // moving links in takes the first link's batch, and that batch's `ON DELETE CASCADE` used
    // to take the package — and the moved links' package — with it once the batch's own links
    // were gone, leaving the rest with no package and out of sight.
    sqlx::query(
        "DELETE FROM collector_batches WHERE NOT EXISTS \
         (SELECT 1 FROM link_candidates WHERE batch_id = collector_batches.id) \
         AND NOT EXISTS (SELECT 1 FROM collector_packages WHERE batch_id = collector_batches.id)",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn get_candidate(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Option<LinkCandidate>> {
    sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

/// Reads the candidates back after the mirror groups were written onto them.
///
/// The rows were built before the grouping ran — it needs the whole package — so the values
/// in hand are one step behind the database. Returning them as they are would hand the
/// caller, and the event it publishes, a batch in which nothing is a mirror of anything.
async fn reread_candidates(
    connection: &mut SqliteConnection,
    candidates: Vec<LinkCandidate>,
) -> Result<Vec<LinkCandidate>> {
    let mut fresh = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let row = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
            .bind(candidate.id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
        match row {
            Some(row) => fresh.push(row.try_into()?),
            None => fresh.push(candidate),
        }
    }
    Ok(fresh)
}

/// Replaces a candidate's enrichment fields; an empty list clears the column.
///
/// A candidate that has already been handed to the queue has no reader of its own left: the
/// enqueue copies the fields it saw when it claimed the row, and detaches the row when it is
/// done. Fields that arrive after that claim therefore have to be carried onto the queue rows
/// the candidate became, or they reach nothing (RD-108-15).
pub(crate) async fn set_enrichment(
    connection: &mut SqliteConnection,
    id: rd_core::CandidateId,
    fields: &[rd_core::EnrichmentField],
) -> Result<()> {
    let stored = if fields.is_empty() {
        None
    } else {
        Some(serde_json::to_string(fields)?)
    };
    let mut tx = connection.begin().await?;
    sqlx::query("UPDATE link_candidates SET enrichment_json = ? WHERE id = ?")
        .bind(stored)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let handed_over: Option<(String, String)> =
        sqlx::query_as("SELECT state, url FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((state, url)) = handed_over
        && matches!(state.as_str(), "resolving" | "enqueued")
    {
        crate::package_store::carry_enrichment_for_source(&mut tx, &url, fields).await?;
    }
    tx.commit().await?;
    Ok(())
}

fn provider_for(url: &Url) -> String {
    rd_provider_registry::provider_for_url(url)
        .map_or_else(|| "direct_http".to_owned(), |spec| spec.slug)
}

/// The writer's own, under the name the collector stores have always imported it by.
pub(crate) use crate::writer::insert_event;

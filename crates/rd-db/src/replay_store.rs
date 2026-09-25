//! Persistence for authenticated request replay: consent on the capture side, the
//! consented template on the transfer side, and the pre-resume refresh budget.
//!
//! The encrypted body's `vault://` reference lives only in the columns written here —
//! `link_candidates.replay_body_ref` and `download_request_templates.body_ref`. It is
//! deliberately absent from every serializable struct, so it cannot reach a REST or SSE
//! payload by widening a type.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{CandidateId, DownloadId, ReplayConsent, RequestTemplate};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// How many pre-resume refreshes one download may spend inside [`REFRESH_WINDOW_HOURS`].
pub const REPLAY_REFRESH_MAX: i64 = 3;
/// Sliding window the refresh budget is counted in.
pub const REFRESH_WINDOW_HOURS: i64 = 24;

/// Reads the consented template of one download.
pub(crate) async fn request_template(
    pool: &SqlitePool,
    id: DownloadId,
) -> Result<Option<RequestTemplate>> {
    let row =
        sqlx::query("SELECT template_json FROM download_request_templates WHERE download_id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    row.map(|row| {
        let json: String = row.try_get("template_json")?;
        serde_json::from_str(&json).context("parse request template")
    })
    .transpose()
}

/// Reads the `vault://` reference of a download's request body.
pub(crate) async fn template_body_ref(pool: &SqlitePool, id: DownloadId) -> Result<Option<String>> {
    let row = sqlx::query("SELECT body_ref FROM download_request_templates WHERE download_id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|row| row.try_get::<Option<String>, _>("body_ref").ok().flatten()))
}

/// Reads a candidate's captured body reference, so ownership can move to the download.
pub(crate) async fn candidate_body_ref(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Option<String>> {
    let row = sqlx::query("SELECT replay_body_ref FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|row| {
        row.try_get::<Option<String>, _>("replay_body_ref")
            .ok()
            .flatten()
    }))
}

/// Reads the consent recorded for a candidate.
pub(crate) async fn candidate_consent(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Option<ReplayConsent>> {
    let row = sqlx::query("SELECT replay_consent_json FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let json: Option<String> = row.try_get("replay_consent_json")?;
    json.map(|json| serde_json::from_str(&json).context("parse replay consent"))
        .transpose()
}

/// Records or clears a candidate's replay consent.
pub(crate) async fn set_candidate_consent(
    connection: &mut SqliteConnection,
    id: CandidateId,
    consent: Option<&ReplayConsent>,
) -> Result<()> {
    let json = consent.map(serde_json::to_string).transpose()?;
    sqlx::query("UPDATE link_candidates SET replay_consent_json = ? WHERE id = ?")
        .bind(json)
        .bind(id.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

/// Writes the consented template of a download and takes ownership of the body reference.
///
/// Runs inside the caller's transaction: the `downloads` row, this row and the hand-over of
/// the candidate's body reference have to commit together, or a crash in between would leave
/// ciphertext in the vault that nothing points at.
pub(crate) async fn insert_request_template(
    connection: &mut SqliteConnection,
    id: DownloadId,
    template: &RequestTemplate,
    body_ref: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO download_request_templates \
         (download_id, version, template_json, body_ref, consent_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(download_id) DO UPDATE SET version = excluded.version, \
         template_json = excluded.template_json, body_ref = excluded.body_ref, \
         consent_json = excluded.consent_json, updated_at = excluded.updated_at",
    )
    .bind(id.to_string())
    .bind(i64::from(template.version))
    .bind(serde_json::to_string(template)?)
    .bind(body_ref)
    .bind(serde_json::to_string(&template.consent)?)
    .bind(now)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// Clears the candidate's body reference once a download owns it.
pub(crate) async fn take_candidate_body_ref(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<()> {
    sqlx::query("UPDATE link_candidates SET replay_body_ref = NULL WHERE id = ?")
        .bind(id.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

/// Atomically reserves one pre-resume refresh.
///
/// Deliberately a different counter from `resolver_refresh_count` (migration 0008): that one
/// is the single reactive retry after a 401/403, and a pre-resume refresh sharing it would
/// leave a genuinely expired credential with no recovery left. Windowed rather than
/// absolute, because a long download that is paused twice legitimately needs more than one.
pub(crate) async fn claim_replay_refresh(
    connection: &mut SqliteConnection,
    id: DownloadId,
    now: DateTime<Utc>,
) -> Result<bool> {
    let window_start = now - chrono::TimeDelta::hours(REFRESH_WINDOW_HOURS);
    let result = sqlx::query(
        "UPDATE downloads \
            SET replay_refresh_count = CASE \
                    WHEN replay_refreshed_at IS NULL OR replay_refreshed_at < ?1 THEN 1 \
                    ELSE replay_refresh_count + 1 END, \
                replay_refreshed_at = ?2, \
                updated_at = ?2 \
          WHERE id = ?3 \
            AND (replay_refreshed_at IS NULL OR replay_refreshed_at < ?1 \
                 OR replay_refresh_count < ?4)",
    )
    .bind(window_start)
    .bind(now)
    .bind(id.to_string())
    .bind(REPLAY_REFRESH_MAX)
    .execute(connection)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Discards a download's partial state so a refreshed URL can start from zero.
///
/// `prepare_transfer` refuses to replace a chunk plan once bytes are committed, which is the
/// right guard for the normal path but exactly what has to be overridden when a refresh
/// produced a URL whose bytes cannot be proven to match the partial.
pub(crate) async fn reset_transfer(
    connection: &mut SqliteConnection,
    id: DownloadId,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("DELETE FROM chunks WHERE download_id = ?")
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?;
    sqlx::query(
        "UPDATE downloads SET committed_bytes = 0, total_bytes = NULL, etag = NULL, \
         last_modified = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(id.to_string())
    .execute(connection)
    .await?;
    Ok(())
}

/// Every `vault://` reference no live candidate or download owns any more.
///
/// Used after a restart to sweep bodies whose owner was deleted while the service was down.
pub(crate) async fn orphaned_body_refs(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT body_ref AS reference FROM download_request_templates \
           WHERE body_ref IS NOT NULL \
             AND download_id NOT IN (SELECT id FROM downloads) \
         UNION \
         SELECT replay_body_ref AS reference FROM link_candidates \
           WHERE replay_body_ref IS NOT NULL \
             AND batch_id NOT IN (SELECT id FROM collector_batches)",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<Option<String>, _>("reference").ok().flatten())
        .collect())
}

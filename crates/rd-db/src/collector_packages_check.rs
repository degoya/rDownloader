//! Online-check claims and results, and renaming a candidate.

use anyhow::{Result, bail};
use chrono::Utc;
use rd_core::{
    CandidateId, EventEnvelope, LinkCandidate, LinkCandidateState, LinkCheckResult, LinkStatus,
};
use sqlx::{Connection, SqliteConnection};

use super::collector_event;
use crate::{
    collector_store::{CandidateRow, GET_CANDIDATE, insert_event},
    enum_string,
    error::StoreError,
};

/// Moves checkable candidates into `checking` and returns them with their *pre-claim*
/// state, so callers can tell duplicates apart and restore that state after the check.
///
/// One transaction for the whole selection (DB-11): a claim is all of it or none, and a large
/// re-check costs one commit rather than one per link.
pub(crate) async fn claim_for_check(
    connection: &mut SqliteConnection,
    ids: &[CandidateId],
) -> Result<Vec<LinkCandidate>> {
    let mut tx = connection.begin().await?;
    let mut claimed = Vec::new();
    for id in ids {
        let Some(row) = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?
        else {
            continue;
        };
        // Fresh intakes already sit in `checking`; claiming them is idempotent. Duplicates
        // are probed too so they get a real file name and media metadata instead of the raw
        // URL segment (a duplicated YouTube link would otherwise stay named "watch").
        let result = sqlx::query(
            "UPDATE link_candidates SET state = 'checking' WHERE id = ? \
             AND state IN ('online', 'offline', 'error', 'unsupported', 'checking', 'duplicate')",
        )
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 1 {
            claimed.push(row.try_into()?);
        }
    }
    tx.commit().await?;
    Ok(claimed)
}

/// Stores the outcome of an online check; only rows still in `checking` are touched.
///
/// `was_duplicate` restores the duplicate state after a successful check: the candidate
/// keeps its warning but gains the probed file name and metadata, so it can be
/// re-downloaded deliberately.
///
/// `cached_by` names the provider whose cache answered (RD-130-11). It is written only
/// together with a `cached_at`: a check that did not end in `cached` clears both, so the
/// two columns always describe the same answer.
pub(crate) async fn record_check(
    connection: &mut SqliteConnection,
    id: CandidateId,
    result: Option<LinkCheckResult>,
    error: Option<rd_core::CandidateMessage>,
    was_duplicate: bool,
    cached_by: Option<String>,
) -> Result<EventEnvelope> {
    let media_json = result
        .as_ref()
        .and_then(|result| result.media.as_ref())
        .map(serde_json::to_string)
        .transpose()?;
    let now = Utc::now();
    // Every check writes the column: a cache answer is only as good as the latest check, so
    // one that did not say "cached" must clear what an earlier one said (RD-120-36).
    let cached_at = result
        .as_ref()
        .is_some_and(|result| result.status == LinkStatus::Cached)
        .then_some(now);
    let cached_by = cached_at.is_some().then_some(cached_by).flatten();
    let (state, file_name, size, message) = match result {
        Some(result) => {
            let state = match result.status {
                LinkStatus::Online | LinkStatus::Unknown | LinkStatus::Cached if was_duplicate => {
                    LinkCandidateState::Duplicate
                }
                LinkStatus::Online | LinkStatus::Cached => LinkCandidateState::Online,
                LinkStatus::Offline => LinkCandidateState::Offline,
                LinkStatus::Unknown => LinkCandidateState::Online,
                // Not folded into the duplicate arm above, and that is the point: `Duplicate`
                // is queueable, so a second copy of a page would go right back to being a
                // queued download (RD-110-07).
                LinkStatus::Unresolvable => LinkCandidateState::Unresolvable,
            };
            let file_name = result
                .file_name
                .map(|name| rd_files::sanitize_file_name(&name))
                .filter(|name| !name.is_empty());
            let size = result
                .size
                .map(|value| i64::try_from(value.get()))
                .transpose()?;
            // Only an inconclusive answer leaves a message behind. A link the check
            // confirmed as online or gone needs none, and keeping the previous one would
            // contradict the state next to it. What the caller passes here is the message
            // for *this* situation — it used to be one fallback sentence handed to every
            // candidate of a batch, which is how `Unknown` came to be reported as a missing
            // result (RD-109-43).
            let message = if matches!(
                result.status,
                LinkStatus::Unknown | LinkStatus::Unresolvable
            ) {
                error
            } else {
                None
            };
            (state, file_name, size, message)
        }
        None => (LinkCandidateState::Error, None, None, error),
    };
    let mut tx = connection.begin().await?;
    sqlx::query(
        // A name the check learned — from a content disposition or the hoster itself — is
        // declared by the source, unlike the one intake took from the address.
        "UPDATE link_candidates SET state = ?, file_name = COALESCE(?, file_name), \
         file_name_declared = CASE WHEN ? IS NULL THEN file_name_declared ELSE 1 END, \
         size = COALESCE(?, size), error = ?, error_code = ?, checked_at = ?, cached_at = ?, \
         cached_by = ?, media_json = COALESCE(?, media_json) \
         WHERE id = ? AND state = 'checking'",
    )
    .bind(enum_string(state)?)
    .bind(file_name.clone())
    .bind(file_name)
    .bind(size)
    .bind(message.as_ref().map(|message| message.text.clone()))
    .bind(message.and_then(|message| message.code))
    .bind(now)
    .bind(cached_at)
    .bind(cached_by)
    .bind(media_json)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    let event = collector_event(serde_json::json!({ "candidate_id": id, "state": state }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Records that no check source exists for a candidate (a hoster link without an account
/// and without a free resolver). Marking it `online` instead would claim the link was
/// verified, which is exactly the state the enqueue path must not trust.
///
/// `cached_by` is the provider whose cache holds the file although nothing here can check
/// it (RD-130-11): the state and the message stay, and the cache answer is stamped next to
/// them, so the person sees that another provider has it. `None` clears an earlier stamp.
pub(crate) async fn mark_unsupported(
    connection: &mut SqliteConnection,
    id: CandidateId,
    message: rd_core::CandidateMessage,
    cached_by: Option<String>,
) -> Result<EventEnvelope> {
    let state = LinkCandidateState::Unsupported;
    let now = Utc::now();
    let cached_at = cached_by.is_some().then_some(now);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "UPDATE link_candidates SET state = ?, error = ?, error_code = ?, checked_at = ?, \
         cached_at = ?, cached_by = ? WHERE id = ? AND state = 'checking'",
    )
    .bind(enum_string(state)?)
    .bind(message.text)
    .bind(message.code)
    .bind(now)
    .bind(cached_at)
    .bind(cached_by)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    let event = collector_event(serde_json::json!({ "candidate_id": id, "state": state }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn set_file_name(
    connection: &mut SqliteConnection,
    id: CandidateId,
    file_name: &str,
) -> Result<(LinkCandidate, EventEnvelope)> {
    let mut tx = connection.begin().await?;
    let result = sqlx::query("UPDATE link_candidates SET file_name = ? WHERE id = ? AND state NOT IN ('resolving', 'enqueued')")
        .bind(file_name).bind(id.to_string()).execute(&mut *tx).await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("link candidate not found or busy"));
    }
    let event = collector_event(serde_json::json!({ "candidate_id": id, "renamed": true }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let candidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    Ok((candidate, event))
}

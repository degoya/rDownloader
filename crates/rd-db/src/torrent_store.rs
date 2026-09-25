//! Persistence of the per-torrent state blobs.
//!
//! Two rows can carry torrent state: a link candidate before the torrent is queued, and
//! the download row afterwards. Both store one typed JSON document, so a change to the
//! file plan, the tracker list or the seeding override is a single atomic write.

use anyhow::{Context, Result};
use rd_core::{CandidateId, DownloadId, EventEnvelope, EventKind, TorrentCandidateState};
use sqlx::{Connection, SqliteConnection};

use crate::{collector_store::insert_event, error::StoreError};

/// Reads the torrent state of one link candidate.
pub(crate) async fn candidate_state(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<Option<TorrentCandidateState>> {
    let stored: Option<Option<String>> =
        sqlx::query_scalar("SELECT torrent_json FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
    stored
        .flatten()
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .context("parse candidate torrent state")
}

/// Replaces the torrent state of one link candidate.
pub(crate) async fn set_candidate_state(
    connection: &mut SqliteConnection,
    id: CandidateId,
    state: &TorrentCandidateState,
) -> Result<EventEnvelope> {
    let mut transaction = connection.begin().await?;
    let affected = sqlx::query("UPDATE link_candidates SET torrent_json = ? WHERE id = ?")
        .bind(serde_json::to_string(state)?)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    anyhow::ensure!(
        affected == 1,
        StoreError::not_found("link candidate not found")
    );
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// Reads the torrent state of one download row.
pub(crate) async fn download_state(
    connection: &mut SqliteConnection,
    id: DownloadId,
) -> Result<Option<rd_core::TorrentJobState>> {
    let stored: Option<Option<String>> =
        sqlx::query_scalar("SELECT torrent_json FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
    stored
        .flatten()
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .context("parse download torrent state")
}

/// Replaces the torrent state of one download row.
///
/// Deliberately emits no event: the runner writes this on every priority-tier change and
/// every seeding tick, which would otherwise flood the SSE stream. Callers that make a
/// user-visible change publish their own event.
/// Persists the torrent state of one download row and announces it.
///
/// The announcement is broadcast only, deliberately: unlike its candidate sibling this write
/// also carries the seeding sweep's bookkeeping, which rewrites the row of every seeding
/// torrent every thirty seconds. Persisting each of those would fill the `events` table with
/// rows nothing reads, the way `DownloadProgress` would if it were persisted. A live client
/// still learns at once that the file plan, the tracker list or the seeding override changed,
/// and a client that reconnects refetches the download anyway.
///
/// The kind is `DownloadState` rather than the `CollectorChanged` its sibling uses: that one
/// is the LinkGrabber's channel and names a candidate, while this row has left the collector.
/// The payload carries no `state`/`previous` pair, so `automation_context` classifies it as
/// no lifecycle moment and it starts no automation run.
pub(crate) async fn set_download_state(
    connection: &mut SqliteConnection,
    id: DownloadId,
    state: &rd_core::TorrentJobState,
) -> Result<EventEnvelope> {
    let affected = sqlx::query("UPDATE downloads SET torrent_json = ? WHERE id = ?")
        .bind(serde_json::to_string(state)?)
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?
        .rows_affected();
    anyhow::ensure!(affected == 1, StoreError::not_found("download not found"));
    Ok(EventEnvelope::new(
        EventKind::DownloadState,
        serde_json::json!({ "download_id": id, "torrent": true }),
    ))
}

/// Reads the torrent state of every download row that has one, for restart recovery.
pub(crate) async fn all_download_states(
    connection: &mut SqliteConnection,
) -> Result<Vec<(DownloadId, rd_core::TorrentJobState)>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, torrent_json FROM downloads WHERE torrent_json IS NOT NULL")
            .fetch_all(&mut *connection)
            .await?;
    rows.into_iter()
        .map(|(id, json)| {
            Ok((
                id.parse().context("parse download id")?,
                serde_json::from_str(&json).context("parse download torrent state")?,
            ))
        })
        .collect()
}

//! Media-specific LinkGrabber operations: playlist fan-out and variant selection.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{
    CandidateId, CollectorPackageId, EventEnvelope, EventKind, LinkCandidate, MediaCandidate,
    MediaCandidateState, MediaInfo, MediaSelectionUpdate,
};
use sqlx::{Connection, SqliteConnection};

use crate::{
    collector_store::{CandidateRow, GET_CANDIDATE, enum_string, insert_event},
    error::{StoreError, StoreErrorKind},
};

/// Appends one candidate per probed playlist entry to the package (skips URLs already known).
pub(crate) async fn add_media_candidates(
    connection: &mut SqliteConnection,
    package_id: CollectorPackageId,
    entries: Vec<MediaCandidate>,
) -> Result<(Vec<LinkCandidate>, EventEnvelope)> {
    let (batch_id, category_id): (String, Option<String>) =
        sqlx::query_as("SELECT batch_id, category_id FROM collector_packages WHERE id = ?")
            .bind(package_id.to_string())
            .fetch_optional(&mut *connection)
            .await?
            .context(StoreError::not_found("collector package not found"))?;
    let mut tx = connection.begin().await?;
    let mut position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), 0) FROM link_candidates WHERE package_id = ?",
    )
    .bind(package_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let mut ids = Vec::with_capacity(entries.len());
    for MediaCandidate { info, state } in entries {
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM link_candidates WHERE url = ? AND package_id = ?)",
        )
        .bind(info.page_url.as_str())
        .bind(package_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if exists != 0 {
            continue;
        }
        position += 1;
        let id = CandidateId::new();
        let file_name = media_file_name(&info);
        let size = info
            .selected_variant()
            .and_then(|variant| variant.filesize_approx)
            .and_then(|value| i64::try_from(value).ok());
        sqlx::query(
            "INSERT INTO link_candidates (id, batch_id, url, state, file_name, size, provider, category_id, priority, package_id, position, checked_at, media_json, media_formats_json, created_at) \
             VALUES (?, ?, ?, 'online', ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&batch_id)
        .bind(info.page_url.as_str())
        .bind(&file_name)
        .bind(size)
        .bind(rd_core::MEDIA_PROVIDER)
        .bind(&category_id)
        .bind(package_id.to_string())
        .bind(position)
        .bind(Utc::now())
        .bind(serde_json::to_string(&info)?)
        .bind(serde_json::to_string(&state)?)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        ids.push(id);
    }
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "package_id": package_id, "added_candidates": ids.len() }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let mut created = Vec::with_capacity(ids.len());
    for id in ids {
        let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
            .bind(id.to_string())
            .fetch_one(&mut *connection)
            .await?
            .try_into()?;
        created.push(candidate);
    }
    Ok((created, event))
}

/// Switches the selected variant of a media candidate (also updates the file name/size).
pub(crate) async fn set_media_variant(
    connection: &mut SqliteConnection,
    id: CandidateId,
    variant_id: &str,
) -> Result<(LinkCandidate, EventEnvelope)> {
    let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .context(StoreError::not_found("link candidate not found"))?
        .try_into()?;
    let Some(mut info) = candidate.media.clone() else {
        bail!(StoreError::new(
            StoreErrorKind::NoMediaMetadata,
            "link candidate has no media metadata"
        ));
    };
    if !info.variants.iter().any(|variant| variant.id == variant_id) {
        bail!(StoreError::new(
            StoreErrorKind::UnknownMediaVariant,
            "unknown media variant"
        ));
    }
    info.selected = variant_id.to_owned();
    let file_name = media_file_name(&info);
    let size = info
        .selected_variant()
        .and_then(|variant| variant.filesize_approx)
        .and_then(|value| i64::try_from(value).ok());
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE link_candidates SET media_json = ?, file_name = ?, size = ? \
         WHERE id = ? AND state NOT IN ('resolving', 'enqueued')",
    )
    .bind(serde_json::to_string(&info)?)
    .bind(&file_name)
    .bind(size)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::busy("link candidate is busy"));
    }
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id, "variant": variant_id }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    let _ = enum_string(candidate.state)?;
    Ok((candidate, event))
}

/// Re-routes a candidate to another provider after the online check learned what it is
/// (RD-080-06).
///
/// A live manifest cannot be told apart from a recorded one by its address, only by its
/// body, so the provider assigned at intake is a guess the check is allowed to correct.
pub(crate) async fn set_provider(
    connection: &mut SqliteConnection,
    id: CandidateId,
    provider: &str,
) -> Result<(LinkCandidate, EventEnvelope)> {
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE link_candidates SET provider = ? \
         WHERE id = ? AND state NOT IN ('enqueued')",
    )
    .bind(provider)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::busy(
            "link candidate is already queued or does not exist"
        ));
    }
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id, "provider": provider }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    Ok((candidate, event))
}

/// Sets the cookie/authentication profile a candidate is queued with (RD-080-04).
///
/// The selection is stored as the same two columns `downloads` uses, so the value survives
/// the hand-off to the queue unchanged rather than being re-derived there.
pub(crate) async fn set_auth_profile(
    connection: &mut SqliteConnection,
    id: CandidateId,
    selection: rd_core::AuthProfileSelection,
) -> Result<(LinkCandidate, EventEnvelope)> {
    let (profile_id, pinned) = selection.to_columns();
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE link_candidates SET auth_profile_id = ?, auth_profile_pinned = ? \
         WHERE id = ? AND state NOT IN ('resolving', 'enqueued')",
    )
    .bind(profile_id.map(|value| value.to_string()))
    .bind(i64::from(pinned))
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::busy("link candidate is busy or does not exist"));
    }
    // The profile id is deliberately not in the payload: an event fans out to every
    // connected client, and which session a link uses is nobody else's business.
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    Ok((candidate, event))
}

/// The format inventory and criteria stored beside a media candidate.
///
/// Returns `None` for a candidate probed before RD-080-01 or for one that is not a media
/// link; the caller then falls back to the bounded variant list on the candidate itself.
pub(crate) async fn candidate_media_state(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<Option<MediaCandidateState>> {
    let stored: Option<Option<String>> =
        sqlx::query_scalar("SELECT media_formats_json FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
    stored
        .flatten()
        .map(|json| serde_json::from_str(&json).context("stored media format inventory"))
        .transpose()
}

/// Stores the probed format inventory beside a media candidate.
///
/// Separate from the link-check result on purpose: the result carries the bounded variant
/// list that every client sees, while the inventory can hold up to
/// [`rd_core::MAX_MEDIA_FORMATS`] entries and is only ever fetched for the one candidate
/// whose selector is open.
pub(crate) async fn set_media_inventory(
    connection: &mut SqliteConnection,
    id: CandidateId,
    state: MediaCandidateState,
) -> Result<()> {
    sqlx::query("UPDATE link_candidates SET media_formats_json = ? WHERE id = ?")
        .bind(serde_json::to_string(&state)?)
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// Stores a resolved format selection on a media candidate.
///
/// The chosen variant replaces any previous entry with the same id and becomes the selected
/// one, so the bounded variant list a client already holds keeps describing what will be
/// downloaded. The criteria are stored alongside the inventory, which is what makes the
/// choice survive a re-probe.
pub(crate) async fn set_media_selection(
    connection: &mut SqliteConnection,
    id: CandidateId,
    update: MediaSelectionUpdate,
) -> Result<(LinkCandidate, EventEnvelope)> {
    let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .context(StoreError::not_found("link candidate not found"))?
        .try_into()?;
    let Some(mut info) = candidate.media.clone() else {
        bail!(StoreError::new(
            StoreErrorKind::NoMediaMetadata,
            "link candidate has no media metadata"
        ));
    };
    let MediaSelectionUpdate { criteria, variant } = update;
    info.variants.retain(|existing| existing.id != variant.id);
    info.selected.clone_from(&variant.id);
    info.variants.push(variant);
    let mut state = candidate_media_state(&mut *connection, id)
        .await?
        .unwrap_or_default();
    state.criteria = criteria;
    let file_name = media_file_name(&info);
    let size = info
        .selected_variant()
        .and_then(|variant| variant.filesize_approx)
        .and_then(|value| i64::try_from(value).ok());
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE link_candidates SET media_json = ?, media_formats_json = ?, file_name = ?, size = ? \
         WHERE id = ? AND state NOT IN ('resolving', 'enqueued')",
    )
    .bind(serde_json::to_string(&info)?)
    .bind(serde_json::to_string(&state)?)
    .bind(&file_name)
    .bind(size)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::busy("link candidate is busy"));
    }
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id, "variant": info.selected }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let candidate: LinkCandidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    let _ = enum_string(candidate.state)?;
    Ok((candidate, event))
}

/// `<sanitised title>.<ext of the selected variant>`.
pub fn media_file_name(info: &MediaInfo) -> String {
    let ext = info
        .selected_variant()
        .map(|variant| variant.ext.clone())
        .unwrap_or_else(|| "mp4".to_owned());
    let stem = rd_files::sanitize_file_name(info.title.trim());
    format!("{stem}.{ext}")
}

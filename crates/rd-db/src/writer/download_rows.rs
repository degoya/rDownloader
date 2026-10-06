//! The writer's own SQL for package, download and chunk rows.
//!
//! The counterpart to [`crate::writer_jobs`], which holds the job-level operations on the same
//! rows. Split out of `writer/mod.rs` so the command arms in [`super::downloads`] and
//! [`super::packages`] stay readable next to the statements they run.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{ByteCount, DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind};
use sqlx::{Connection, Row};

use crate::error::StoreError;
use crate::models::{NewDownload, NewPackage};

use super::{Writer, insert_event};

#[path = "download_rows_progress.rs"]
mod progress;

/// How many ids an event about many rows names at most; `count` says how many there were.
const ANNOUNCED_IDS: usize = 100;

/// One `download.state` event about many rows (RD-1120-17): `fields` plus the `count`, the
/// first [`ANNOUNCED_IDS`] `download_ids` and, for a single row, its `download_id`, as every
/// other `download.state` event names its row. One event per batch keeps a large enqueue or
/// removal from overrunning the 512 live slots every subscriber has.
pub(crate) fn rows_event(ids: &[DownloadId], mut fields: serde_json::Value) -> EventEnvelope {
    if let Some(object) = fields.as_object_mut() {
        object.insert("count".to_owned(), serde_json::json!(ids.len()));
        object.insert(
            "download_ids".to_owned(),
            serde_json::json!(ids.iter().take(ANNOUNCED_IDS).collect::<Vec<_>>()),
        );
        if let [only] = ids {
            object.insert("download_id".to_owned(), serde_json::json!(only));
        }
    }
    EventEnvelope::new(EventKind::DownloadState, fields)
}

impl Writer {
    pub(super) async fn create_package(
        &mut self,
        package: NewPackage,
    ) -> Result<rd_core::DownloadPackage> {
        let now = Utc::now();
        let position: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) + 1 FROM packages")
                .fetch_one(&mut self.connection)
                .await?;
        sqlx::query(
            "INSERT INTO packages (id, name, state, destination, category_id, priority, position, \
             postprocess_level, script, enrichment_json, created_at, updated_at) \
             VALUES (?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(package.id.to_string())
        .bind(&package.name)
        .bind(&package.destination)
        .bind(package.category_id.map(|id| id.to_string()))
        .bind(package.priority.as_i32())
        .bind(position)
        .bind(
            package
                .postprocess_level
                .map(crate::enum_string)
                .transpose()?,
        )
        .bind(&package.script)
        .bind(crate::package_store::encode_enrichment(
            &package.enrichment,
        )?)
        .bind(now)
        .bind(now)
        .execute(&mut self.connection)
        .await?;

        Ok(rd_core::DownloadPackage {
            id: package.id,
            name: package.name,
            state: rd_core::PackageState::Queued,
            created_at: now,
            destination: package.destination,
            category_id: package.category_id,
            priority: package.priority,
            position,
            has_password: false,
            password: None,
            kind: rd_core::DownloadKind::Http,
            nzb_import_id: None,
            completed_at: None,
            postprocess_level: package.postprocess_level,
            script: package.script,
            postprocess: rd_core::PostprocessStatus::default(),
            extraction_result: None,
            // Filled after the enqueue, once the candidates behind the package are known.
            enrichment: Vec::new(),
        })
    }

    pub(super) async fn create_download(
        &mut self,
        download: NewDownload,
        sources: Option<Box<rd_core::SourceSet>>,
    ) -> Result<DownloadFile> {
        if !matches!(
            download.initial_state,
            // Skipped joins these two because a mirror is held back from the moment it is
            // created: it never passes through the queue on its way to waiting.
            DownloadState::Queued | DownloadState::Paused | DownloadState::Skipped
        ) {
            bail!("new download must start queued, paused or skipped");
        }
        let now = Utc::now();
        let mut tx = self.connection.begin().await?;
        let position = insert_download_row(&mut tx, &download, now).await?;
        attach_download_records(&mut tx, &download, sources.as_deref(), now).await?;
        tx.commit().await?;
        Ok(new_download_file(download, position, now))
    }

    /// Announces the rows one enqueue created, with one event for all of them (RD-1120-17), so
    /// a list open elsewhere shows them without a reload; they used to wait for their first
    /// transition, which a paused row or a full queue may not have for hours.
    ///
    /// One event per enqueue rather than per row: a LinkGrabber hand-over of a thousand links
    /// would otherwise overrun every subscriber's 512 slots, and each browser would fall back
    /// to a resynchronisation in exactly the common case. The payload is [`rows_event`]'s with
    /// the package. No `state`/`previous` pair, as with `joined_queue`: nothing moved, so
    /// automations and notifications see no transition.
    pub(super) async fn announce_created(
        &mut self,
        package_id: rd_core::PackageId,
        ids: &[DownloadId],
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let event = rows_event(
            ids,
            serde_json::json!({ "package_id": package_id, "created": true }),
        );
        let mut tx = self.connection.begin().await?;
        insert_event(&mut tx, &event).await?;
        tx.commit().await?;
        let _ = self.events.send(event);
        Ok(())
    }

    pub(super) async fn transition_download(
        &mut self,
        id: DownloadId,
        next: DownloadState,
    ) -> Result<DownloadFile> {
        self.transition_download_with_reason(id, next, None).await
    }

    /// Queues a row the enqueue wrote paused, the step that follows a torrent's reviewed
    /// selection (API-07).
    ///
    /// Conditional, and without a lifecycle event (re-audit 1.9.1, RA-TR-08, RA-API-06): the
    /// row moves only while it is still `paused` and untouched since `created_at`, so a pause
    /// or resume somebody gave it in between stands. The event carries no `state`/`previous`
    /// pair, like the selection's own: a row that was created queued announces no transition
    /// either, so automations and notifications see none here, while a client refetches.
    pub(super) async fn join_queue(
        &mut self,
        id: DownloadId,
        created_at: chrono::DateTime<Utc>,
    ) -> Result<DownloadFile> {
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "joined_queue": true }),
        );
        let mut tx = self.connection.begin().await?;
        let joined = sqlx::query(
            "UPDATE downloads SET state = ?, updated_at = ? \
             WHERE id = ? AND state = ? AND updated_at = ?",
        )
        .bind(DownloadState::Queued.to_string())
        .bind(event.occurred_at)
        .bind(id.to_string())
        .bind(DownloadState::Paused.to_string())
        .bind(created_at)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if joined {
            insert_event(&mut tx, &event).await?;
        }
        tx.commit().await?;
        if joined {
            let _ = self.events.send(event);
        }
        crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))
    }

    /// Blocks a download and records the cause on the row.
    ///
    /// The cause has to survive the process, because the release paths are not
    /// interchangeable: freeing disk space must restart what the full disk stopped and must
    /// leave a transfer whose validators changed mid-flight exactly where it is.
    pub(super) async fn block_download(
        &mut self,
        id: DownloadId,
        reason: String,
    ) -> Result<DownloadFile> {
        self.transition_download_with_reason(id, DownloadState::Blocked, Some(reason))
            .await
    }

    /// The one transition path. `reason` is written to `block_reason` and cleared by every
    /// transition that does not carry one, so the column never outlives the block it explains.
    async fn transition_download_with_reason(
        &mut self,
        id: DownloadId,
        next: DownloadState,
        reason: Option<String>,
    ) -> Result<DownloadFile> {
        let row = sqlx::query("SELECT state FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut self.connection)
            .await?
            .context(StoreError::not_found("download not found"))?;
        let current: DownloadState = row.get::<String, _>("state").parse()?;
        // Tagged, so a pause or resume the state does not allow reaches the interface as a
        // refusal instead of an internal error (RD-150-22).
        if !current.can_transition_to(next) {
            bail!(StoreError::wrong_state(format!(
                "invalid download transition {current} -> {next}"
            )));
        }

        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "previous": current, "state": next }),
        );
        let mut tx = self.connection.begin().await?;
        sqlx::query(
            "UPDATE downloads SET state = ?, \
             last_error_json = CASE WHEN ? = 'queued' THEN NULL ELSE last_error_json END, \
             block_reason = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(next.to_string())
        .bind(next.to_string())
        .bind(reason)
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        insert_event(&mut tx, &event).await?;
        tx.commit().await?;
        let _ = self.events.send(event);

        let updated = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context("download disappeared after transition")?;
        // May itself decide a held-back PAR2 verdict, including this row's own when the
        // transition that just happened was the move into `Verifying` (RD-108-24), so the
        // row returned here is the one before that second write.
        self.settle_package_after_download(updated.package_id)
            .await?;
        Ok(updated)
    }
}

/// Writes the `downloads` row of a new download at the end of its package; returns its position.
async fn insert_download_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    download: &NewDownload,
    now: chrono::DateTime<Utc>,
) -> Result<i64> {
    let (algorithm, checksum) = download
        .expected_checksum
        .as_ref()
        .map(|value| {
            let algorithm = serde_json::to_string(&value.algorithm)
                .map(|encoded| encoded.trim_matches('"').to_owned());
            algorithm.map(|algorithm| (Some(algorithm), Some(value.value.clone())))
        })
        .transpose()?
        .unwrap_or((None, None));

    let (auth_profile_id, auth_profile_pinned) = download.auth_profile.to_columns();
    let position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM downloads WHERE package_id = ?",
    )
    .bind(download.package_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO downloads (id, package_id, source_url, file_name, state, total_bytes, \
         committed_bytes, checksum_algorithm, checksum_value, account_id, proxy_profile_id, \
         auth_profile_id, auth_profile_pinned, position, kind, media_json, \
         remote_credential_id, mirror_group, enrichment_json, secret_fragment_ref, \
         created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(download.id.to_string())
    .bind(download.package_id.to_string())
    .bind(download.source.as_str())
    .bind(&download.file_name)
    .bind(download.initial_state.to_string())
    .bind(download.total_bytes.map(|value| value.get() as i64))
    .bind(algorithm)
    .bind(checksum)
    .bind(download.account_id.map(|id| id.to_string()))
    .bind(download.proxy_profile_id.map(|id| id.to_string()))
    .bind(auth_profile_id.map(|id| id.to_string()))
    .bind(auth_profile_pinned)
    .bind(position)
    .bind(crate::enum_string(download.kind)?)
    .bind(
        download
            .media
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    .bind(download.remote_credential_id.map(|id| id.to_string()))
    .bind(download.mirror_group.clone())
    .bind(crate::package_store::encode_enrichment(
        &download.enrichment,
    )?)
    .bind(
        download
            .secret_fragment
            .as_ref()
            .map(|fragment| fragment.reference.clone()),
    )
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(position)
}

/// Writes what belongs to a new download row inside the caller's transaction: the request
/// template, the hand-over of the candidate's references, and the Metalink sources.
async fn attach_download_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    download: &NewDownload,
    sources: Option<&rd_core::SourceSet>,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
    // The download row, its template and the hand-over of the candidate's body
    // reference commit together. A crash between them would leave ciphertext in the
    // vault that nothing points at, or a template whose download never existed.
    if let Some(replay) = &download.replay {
        let template = rd_core::RequestTemplate {
            version: rd_core::REQUEST_TEMPLATE_VERSION,
            request: replay.request.clone(),
            consent: replay.consent.clone(),
        };
        crate::replay_store::insert_request_template(
            tx,
            download.id,
            &template,
            replay.body_ref.as_deref(),
            now,
        )
        .await?;
        if let Some(candidate_id) = replay.candidate_id {
            crate::replay_store::take_candidate_body_ref(tx, candidate_id).await?;
        }
    }
    // Same rule for the vaulted link fragment (RD-110-38): the download row now owns the
    // reference, so the candidate must stop pointing at it in the same transaction --
    // otherwise deleting the candidate later would remove a secret the queue still needs.
    if let Some(fragment) = &download.secret_fragment
        && let Some(candidate_id) = fragment.candidate_id
    {
        crate::collector_store::take_candidate_secret_fragment_ref(tx, candidate_id).await?;
    }

    // The sources of a Metalink file belong to the row from its first moment (RD-150-03):
    // a transfer that started between two writes would take one mirror for the whole set.
    if let Some(set) = sources {
        crate::download_sources_store::insert_set(tx, download.id, set, now).await?;
    }
    Ok(())
}

/// The queue row a freshly written download reads back as.
fn new_download_file(
    download: NewDownload,
    position: i64,
    now: chrono::DateTime<Utc>,
) -> DownloadFile {
    DownloadFile {
        recording: None,
        id: download.id,
        package_id: download.package_id,
        source: download.source,
        file_name: download.file_name,
        state: download.initial_state,
        total_bytes: download.total_bytes,
        committed_bytes: ByteCount::default(),
        retry_count: 0,
        next_retry_at: None,
        expected_checksum: download.expected_checksum,
        computed_checksum: None,
        last_error: None,
        account_id: download.account_id,
        proxy_profile_id: download.proxy_profile_id,
        remote_credential_id: download.remote_credential_id,
        mirror_group: download.mirror_group.clone(),
        auth_profile: download.auth_profile,
        position,
        kind: download.kind,
        nzb_file_id: None,
        // Recovery data is decided by the NZB when the import is queued
        // (`nzb_queue::enqueue_import`); nothing on this path enqueues one.
        recovery: false,
        media: download.media,
        // Carried over from the candidate after the enqueue, not at insert time: the
        // scheduler builds the rows before anything knows which candidate became which.
        enrichment: Vec::new(),
        created_at: now,
        updated_at: now,
    }
}

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{
    ByteCount, EventEnvelope, EventKind, NzbFileId, NzbFileStatus, NzbImport, NzbImportId,
    NzbImportState, NzbSegmentId, NzbSegmentState, NzbSegmentStatus,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, parse_id, writer::insert_event};

/// Validated NZB data ready for serialized persistence.
pub struct NewNzbImport {
    pub name: String,
    pub sha256: String,
    /// A category somebody chose for this very intake: the hotfolder's own setting, the
    /// upload form's field, the SABnzbd `cat` parameter. `None` hands the decision to the
    /// routing rules.
    pub category_id: Option<rd_core::CategoryId>,
    /// Where this NZB came in from, so a routing rule can target it.
    ///
    /// A hotfolder drop reports `HotFolder`, an upload `Manual`, the SABnzbd adapter `Api`,
    /// and an NZB fetched behind a LinkGrabber link `Nzb`.
    pub source: rd_core::IngressSource,
    /// Queue priority chosen at import time; `None` = decided when enqueueing.
    pub priority: Option<rd_core::DownloadPriority>,
    pub import_mode: rd_core::ImportMode,
    pub source_path: Option<String>,
    /// Archive password taken from the `{{password}}` file-name convention, its intake package,
    /// or `<head><meta type="password">` inside the NZB.
    pub password: Option<String>,
    pub files: Vec<NewNzbFile>,
    /// Whether this import is something arriving, as opposed to a link already in hand.
    ///
    /// A hotfolder drop or an upload is news. An NZB the online check recognised behind a link
    /// is not: those links were announced when they entered the LinkGrabber, and announcing the
    /// import as well reports the files inside one NZB as though they were links somebody just
    /// added — several times over for a paste that held several of them.
    pub announce_arrival: bool,
}

/// An NZB that could not be taken in, ready to be recorded as a failed import.
///
/// Deliberately not a variant of [`NewNzbImport`]: there are no files, no segments and no
/// routing decision to take - everything that is known about it is the file it came from and
/// why it did not work.
pub struct FailedNzbImport {
    /// The sanitised file name, as the successful path would have stored it.
    pub name: String,
    pub sha256: String,
    pub source_path: Option<String>,
    /// Why the NZB could not be taken in. One line of English, already bounded by the caller.
    pub error: String,
}

/// Editable routing metadata of an NZB that is still waiting in the LinkGrabber.
#[derive(Clone, Debug, Default)]
pub struct NzbImportChange {
    /// `None` leaves the category untouched; `Some(None)` selects the default category.
    pub category_id: Option<Option<rd_core::CategoryId>>,
    pub priority: Option<rd_core::DownloadPriority>,
}

pub struct NewNzbFile {
    pub subject: String,
    pub poster: String,
    pub groups: Vec<String>,
    pub segments: Vec<NewNzbSegment>,
}

pub struct NewNzbSegment {
    pub number: u32,
    pub bytes: u64,
    pub message_id: String,
}

pub(crate) async fn add_import(
    connection: &mut SqliteConnection,
    new: NewNzbImport,
) -> Result<(NzbImport, Vec<EventEnvelope>)> {
    let announce_arrival = new.announce_arrival;
    let category_id = resolve_category(connection, &new).await?;
    if let Some(mut existing) = get_by_hash_connection(connection, &new.sha256).await? {
        existing.duplicate = true;
        // The same file arriving again is routed again: a folder whose category changed, or a
        // drop into a different folder, must not keep what the first arrival decided. Only a
        // resolved category overwrites - `None` is no decision - and an import that already
        // became a package is left alone, because its routing has been acted on.
        if existing.state != NzbImportState::Enqueued
            && let Some(category_id) =
                category_id.filter(|value| Some(*value) != existing.category_id)
        {
            let event = EventEnvelope::new(
                EventKind::CollectorChanged,
                serde_json::json!({ "nzb_import_id": existing.id, "updated": true }),
            );
            let mut transaction = connection.begin().await?;
            sqlx::query("UPDATE nzb_imports SET category_id = ?, updated_at = ? WHERE id = ?")
                .bind(category_id.to_string())
                .bind(event.occurred_at)
                .bind(existing.id.to_string())
                .execute(&mut *transaction)
                .await?;
            insert_event(&mut transaction, &event).await?;
            transaction.commit().await?;
            existing.category_id = Some(category_id);
            return Ok((existing, vec![event]));
        }
        // A container that is already here is not an intake and must not announce itself.
        return Ok((existing, Vec::new()));
    }
    let file_count = u32::try_from(new.files.len()).context("too many NZB files")?;
    let segment_count = new.files.iter().try_fold(0_u32, |total, file| {
        total
            .checked_add(u32::try_from(file.segments.len())?)
            .context("too many NZB segments")
    })?;
    let total_bytes = new.files.iter().try_fold(0_u64, |file_total, file| {
        file.segments.iter().try_fold(file_total, |total, segment| {
            total
                .checked_add(segment.bytes)
                .context("NZB size overflow")
        })
    })?;
    let total_bytes = ByteCount::new(total_bytes).map_err(anyhow::Error::msg)?;
    let mut transaction = connection.begin().await?;
    // Read inside the transaction, so a concurrent insert cannot see the same maximum and place
    // two LinkGrabber rows at one position.
    let position = crate::collector_packages::next_grabber_position(&mut transaction).await?;
    let import = NzbImport {
        id: NzbImportId::new(),
        name: new.name,
        sha256: new.sha256,
        // Enqueue mode is applied by the caller (it knows the destination) via `enqueue_import`.
        state: NzbImportState::Imported,
        file_count,
        segment_count,
        total_bytes,
        category_id,
        priority: new.priority,
        import_mode: new.import_mode,
        source_path: new.source_path,
        error: None,
        duplicate: false,
        has_password: new.password.is_some(),
        password: new.password.clone(),
        position,
        created_at: Utc::now(),
    };
    sqlx::query(
        "INSERT INTO nzb_imports (id, name, sha256, state, file_count, segment_count, total_bytes, category_id, priority, import_mode, source_path, password, position, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(import.id.to_string())
    .bind(&import.name)
    .bind(&import.sha256)
    .bind(enum_string(import.state)?)
    .bind(i64::from(import.file_count))
    .bind(i64::from(import.segment_count))
    .bind(i64::try_from(import.total_bytes.get())?)
    .bind(import.category_id.map(|id| id.to_string()))
    .bind(import.priority.map(|value| i64::from(value.as_i32())))
    .bind(enum_string(import.import_mode)?)
    .bind(&import.source_path)
    .bind(&new.password)
    .bind(import.position)
    .bind(import.created_at)
    .bind(import.created_at)
    .execute(&mut *transaction)
    .await?;
    for (ordinal, file) in new.files.into_iter().enumerate() {
        let file_id = NzbFileId::new();
        let file_bytes = file.segments.iter().try_fold(0_u64, |total, segment| {
            total
                .checked_add(segment.bytes)
                .context("NZB file size overflow")
        })?;
        sqlx::query(
            "INSERT INTO nzb_files (id, import_id, subject, poster, groups_json, total_bytes, ordinal) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(file_id.to_string())
        .bind(import.id.to_string())
        .bind(file.subject)
        .bind(file.poster)
        .bind(serde_json::to_string(&file.groups)?)
        .bind(i64::try_from(file_bytes)?)
        .bind(i64::try_from(ordinal)?)
        .execute(&mut *transaction)
        .await?;
        for segment in file.segments {
            sqlx::query(
                "INSERT INTO nzb_segments (id, file_id, number, bytes, message_id) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(file_id.to_string())
            .bind(i64::from(segment.number))
            .bind(i64::try_from(segment.bytes)?)
            .bind(segment.message_id)
            .execute(&mut *transaction)
            .await?;
        }
    }
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "nzb_import_id": import.id, "file_count": import.file_count }),
    );
    insert_event(&mut transaction, &event).await?;
    // An NZB that arrived on its own is an intake like any other; the desktop agent hangs its
    // notification off this rather than off `collector.changed`, which also fires for every
    // later edit. One recognised behind a link stays silent: those links were already
    // announced, and the count here is files inside the NZB, not links anybody added.
    let mut events = vec![event];
    if announce_arrival {
        let intake = EventEnvelope::new(
            EventKind::CollectorIntake,
            serde_json::json!({
                "nzb_import_id": import.id,
                "file_count": import.file_count,
                "package_count": 1,
                "source": rd_core::IngressSource::Nzb,
            }),
        );
        insert_event(&mut transaction, &intake).await?;
        events.push(intake);
    }
    transaction.commit().await?;
    Ok((import, events))
}

/// Records an NZB that could not be taken in, so the drop is findable with its reason.
///
/// The row is the trace, and it is the same trace the rest of the application already leaves
/// for an unattended job that fails: a subscription keeps its `last_error`, so does an indexer
/// and so does a stream channel, and each is shown on the row it belongs to. `nzb_imports`
/// already carries `last_error` and the LinkGrabber already renders `state = failed` with the
/// reason beneath it (RD-108-20) - what was missing was only the write.
///
/// A second failure of the same file updates the reason instead of adding a row, because
/// `sha256` is unique and the same bytes are the same drop. An import that already became a
/// package is left alone: it worked, and a later refusal does not retract that.
pub(crate) async fn record_import_failure(
    connection: &mut SqliteConnection,
    failed: FailedNzbImport,
) -> Result<(NzbImport, Option<EventEnvelope>)> {
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "nzb_import_failed": true, "sha256": failed.sha256 }),
    );
    let mut transaction = connection.begin().await?;
    if let Some(existing) = get_by_hash_connection(&mut transaction, &failed.sha256).await? {
        if existing.state == NzbImportState::Enqueued {
            transaction.commit().await?;
            return Ok((existing, None));
        }
        sqlx::query(
            "UPDATE nzb_imports SET state = ?, last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(enum_string(NzbImportState::Failed)?)
        .bind(&failed.error)
        .bind(event.occurred_at)
        .bind(existing.id.to_string())
        .execute(&mut *transaction)
        .await?;
        insert_event(&mut transaction, &event).await?;
        let updated = get_by_id_connection(&mut transaction, existing.id)
            .await?
            .context(StoreError::not_found("NZB import not found"))?;
        transaction.commit().await?;
        return Ok((updated, Some(event)));
    }
    let position = crate::collector_packages::next_grabber_position(&mut transaction).await?;
    let import = NzbImport {
        id: NzbImportId::new(),
        name: failed.name,
        sha256: failed.sha256,
        state: NzbImportState::Failed,
        file_count: 0,
        segment_count: 0,
        total_bytes: ByteCount::new(0).map_err(anyhow::Error::msg)?,
        // No category: routing decides where files go, and this one brought none.
        category_id: None,
        priority: None,
        // Nothing can be queued from it, whatever the folder was set to.
        import_mode: rd_core::ImportMode::Review,
        source_path: failed.source_path,
        error: Some(failed.error.clone()),
        duplicate: false,
        has_password: false,
        password: None,
        position,
        created_at: event.occurred_at,
    };
    sqlx::query(
        "INSERT INTO nzb_imports (id, name, sha256, state, file_count, segment_count, total_bytes, import_mode, source_path, last_error, position, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 0, 0, 0, ?, ?, ?, ?, ?, ?)",
    )
    .bind(import.id.to_string())
    .bind(&import.name)
    .bind(&import.sha256)
    .bind(enum_string(import.state)?)
    .bind(enum_string(import.import_mode)?)
    .bind(&import.source_path)
    .bind(&failed.error)
    .bind(import.position)
    .bind(import.created_at)
    .bind(import.created_at)
    .execute(&mut *transaction)
    .await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((import, Some(event)))
}

/// Chooses the category of an arriving NZB, with the same rules the candidate path applies.
///
/// Precedence, and the reason for it: a category somebody chose for this intake wins - the
/// hotfolder's own setting, the upload form's field, the `cat` a SABnzbd client sent - because
/// it is a decision made about this very file. Only when nobody chose one do the routing rules
/// get their say, and the category marked as default is the last resort. That is the order
/// `collector_store::add_batch` already applies to links, and an NZB file is no reason to rank
/// the sources of a decision differently.
async fn resolve_category(
    connection: &mut SqliteConnection,
    new: &NewNzbImport,
) -> Result<Option<rd_core::CategoryId>> {
    if let Some(category_id) = new.category_id {
        return Ok(Some(category_id));
    }
    let (rules, default_category) = crate::config_store::routing_config(connection).await?;
    let url = routing_url(new.source_path.as_deref())?;
    Ok(rd_collector::select_category(
        &rules,
        &rd_collector::CategoryContext {
            source: new.source,
            url: &url,
            file_name: Some(new.name.as_str()),
            mime_type: None,
        },
        default_category,
    ))
}

/// The address a rule's `domain` and `protocol` conditions are matched against.
///
/// An NZB fetched behind a link carries the redacted address it came from, a hotfolder drop the
/// file it was read from, an upload nothing at all. The file path is tried first: a Windows path
/// begins with a drive letter, which `Url::parse` would read as a scheme.
fn routing_url(source_path: Option<&str>) -> Result<Url> {
    if let Some(value) = source_path.map(str::trim).filter(|value| !value.is_empty()) {
        if let Ok(url) = Url::from_file_path(value) {
            return Ok(url);
        }
        if let Ok(url) = Url::parse(value) {
            return Ok(url);
        }
    }
    // Well-formed and deliberately matching no host: a rule that names a domain or a protocol
    // is asking about something an upload does not have.
    Ok(Url::parse("file:///")?)
}

pub(crate) async fn list_imports(pool: &SqlitePool) -> Result<Vec<NzbImport>> {
    // The LinkGrabber's manual order is one sequence over both tables, so this list has to come
    // out of the database in it; sorting by creation time in the client is what made an import
    // un-draggable in the first place.
    sqlx::query_as::<_, NzbImportRow>(&format!(
        "{IMPORT_SELECT} ORDER BY position ASC, created_at ASC"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn update_import(
    connection: &mut SqliteConnection,
    id: NzbImportId,
    change: NzbImportChange,
) -> Result<(NzbImport, EventEnvelope)> {
    anyhow::ensure!(
        change.category_id.is_some() || change.priority.is_some(),
        "no NZB import change specified"
    );
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "nzb_import_id": id, "updated": true }),
    );
    let mut transaction = connection.begin().await?;
    let state: String = sqlx::query_scalar("SELECT state FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    anyhow::ensure!(
        state != "enqueued",
        StoreError::wrong_state("enqueued NZB import cannot be changed")
    );

    if let Some(category_id) = change.category_id {
        sqlx::query("UPDATE nzb_imports SET category_id = ?, updated_at = ? WHERE id = ?")
            .bind(category_id.map(|value| value.to_string()))
            .bind(event.occurred_at)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    if let Some(priority) = change.priority {
        sqlx::query("UPDATE nzb_imports SET priority = ?, updated_at = ? WHERE id = ?")
            .bind(i64::from(priority.as_i32()))
            .bind(event.occurred_at)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    let updated = get_by_id_connection(&mut transaction, id)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((updated, event))
}

pub(crate) async fn list_files(
    pool: &SqlitePool,
    import_id: NzbImportId,
) -> Result<Vec<NzbFileStatus>> {
    let files = sqlx::query_as::<_, NzbFileRow>(
        "SELECT id, import_id, subject, poster, groups_json, total_bytes, ordinal, output_path, \
         assembly_name, declared_size \
         FROM nzb_files WHERE import_id = ? ORDER BY ordinal",
    )
    .bind(import_id.to_string())
    .fetch_all(pool)
    .await?;
    let mut result = Vec::with_capacity(files.len());
    for file in files {
        let file_id: NzbFileId = parse_id(&file.id)?;
        let segments = sqlx::query_as::<_, NzbSegmentRow>(
            "SELECT id, number, bytes, message_id, state, server_attempts, crc32, \
             part_begin, part_end \
             FROM nzb_segments WHERE file_id = ? ORDER BY number",
        )
        .bind(file_id.to_string())
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()?;
        result.push(NzbFileStatus {
            id: file_id,
            import_id: parse_id(&file.import_id)?,
            subject: file.subject,
            poster: file.poster,
            groups: serde_json::from_str(&file.groups_json)?,
            total_bytes: ByteCount::new(u64::try_from(file.total_bytes)?)
                .map_err(anyhow::Error::msg)?,
            ordinal: u32::try_from(file.ordinal)?,
            output_path: file.output_path,
            assembly_name: file.assembly_name,
            declared_size: file
                .declared_size
                .map(u64::try_from)
                .transpose()?
                .map(ByteCount::new)
                .transpose()
                .map_err(anyhow::Error::msg)?,
            segments,
        });
    }
    Ok(result)
}

pub(crate) async fn delete_import(
    connection: &mut SqliteConnection,
    id: NzbImportId,
) -> Result<EventEnvelope> {
    sqlx::query_scalar::<_, String>("SELECT state FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM packages WHERE nzb_import_id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?;
    anyhow::ensure!(
        queued == 0,
        StoreError::wrong_state(
            "active NZB import cannot be removed (delete the package in the downloader instead)"
        )
    );
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "nzb_import_id": id, "removed": true }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// Drops a completed package's NZB import history while keeping the package itself
/// (`packages.nzb_import_id` is `ON DELETE SET NULL`, files/segments cascade).
/// No-op for packages without an import link.
pub(crate) async fn forget_import_for_package(
    connection: &mut SqliteConnection,
    package_id: rd_core::PackageId,
) -> Result<Option<EventEnvelope>> {
    let import_id: Option<String> =
        sqlx::query_scalar("SELECT nzb_import_id FROM packages WHERE id = ?")
            .bind(package_id.to_string())
            .fetch_optional(&mut *connection)
            .await?
            .flatten();
    let Some(import_id) = import_id else {
        return Ok(None);
    };
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "nzb_import_id": import_id, "removed": true }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM nzb_imports WHERE id = ?")
        .bind(&import_id)
        .execute(&mut *transaction)
        .await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(Some(event))
}

pub(crate) async fn set_segment_state(
    connection: &mut SqliteConnection,
    id: NzbSegmentId,
    state: NzbSegmentState,
    crc32: Option<u32>,
) -> Result<EventEnvelope> {
    if state == NzbSegmentState::Completed {
        anyhow::ensure!(crc32.is_some(), "completed NZB segment requires CRC32");
    }
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "nzb_segment_id": id, "state": state }),
    );
    let attempts_increment = i64::from(state == NzbSegmentState::Downloading);
    let mut transaction = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE nzb_segments SET state = ?, server_attempts = server_attempts + ?, crc32 = ? \
         , part_begin = CASE WHEN ? = 'completed' THEN part_begin ELSE NULL END \
         , part_end = CASE WHEN ? = 'completed' THEN part_end ELSE NULL END \
         WHERE id = ?",
    )
    .bind(enum_string(state)?)
    .bind(attempts_increment)
    .bind(crc32.map(i64::from))
    .bind(enum_string(state)?)
    .bind(enum_string(state)?)
    .bind(id.to_string())
    .execute(&mut *transaction)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        StoreError::not_found("NZB segment not found")
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

#[derive(FromRow)]
struct NzbFileRow {
    id: String,
    import_id: String,
    subject: String,
    poster: String,
    groups_json: String,
    total_bytes: i64,
    ordinal: i64,
    output_path: Option<String>,
    assembly_name: Option<String>,
    declared_size: Option<i64>,
}

#[derive(FromRow)]
struct NzbSegmentRow {
    id: String,
    number: i64,
    bytes: i64,
    message_id: String,
    state: String,
    server_attempts: i64,
    crc32: Option<i64>,
    part_begin: Option<i64>,
    part_end: Option<i64>,
}

impl TryFrom<NzbSegmentRow> for NzbSegmentStatus {
    type Error = anyhow::Error;

    fn try_from(row: NzbSegmentRow) -> Result<Self> {
        Ok(Self {
            id: parse_id::<NzbSegmentId>(&row.id)?,
            number: u32::try_from(row.number)?,
            bytes: ByteCount::new(u64::try_from(row.bytes)?).map_err(anyhow::Error::msg)?,
            message_id: row.message_id,
            state: serde_json::from_str(&format!("\"{}\"", row.state))?,
            server_attempts: u32::try_from(row.server_attempts)?,
            crc32: row
                .crc32
                .map(u32::try_from)
                .transpose()?
                .map(|value| format!("{value:08x}")),
            part_begin: optional_bytes(row.part_begin)?,
            part_end: optional_bytes(row.part_end)?,
        })
    }
}

fn optional_bytes(value: Option<i64>) -> Result<Option<ByteCount>> {
    value
        .map(u64::try_from)
        .transpose()?
        .map(ByteCount::new)
        .transpose()
        .map_err(anyhow::Error::msg)
}

async fn get_by_hash_connection(
    connection: &mut SqliteConnection,
    sha256: &str,
) -> Result<Option<NzbImport>> {
    sqlx::query_as::<_, NzbImportRow>(&format!("{IMPORT_SELECT} WHERE sha256 = ?"))
        .bind(sha256)
        .fetch_optional(connection)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

async fn get_by_id_connection(
    connection: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: NzbImportId,
) -> Result<Option<NzbImport>> {
    sqlx::query_as::<_, NzbImportRow>(&format!("{IMPORT_SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(&mut **connection)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

const IMPORT_SELECT: &str = "SELECT id, name, sha256, state, file_count, segment_count, total_bytes, category_id, priority, import_mode, source_path, last_error, password IS NOT NULL AS has_password, password, position, created_at FROM nzb_imports";

#[derive(FromRow)]
struct NzbImportRow {
    id: String,
    name: String,
    sha256: String,
    state: String,
    file_count: i64,
    segment_count: i64,
    total_bytes: i64,
    category_id: Option<String>,
    priority: Option<i64>,
    import_mode: String,
    source_path: Option<String>,
    last_error: Option<String>,
    has_password: i64,
    password: Option<String>,
    position: i64,
    created_at: DateTime<Utc>,
}

impl TryFrom<NzbImportRow> for NzbImport {
    type Error = anyhow::Error;

    fn try_from(row: NzbImportRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            sha256: row.sha256,
            state: serde_json::from_str(&format!("\"{}\"", row.state))?,
            file_count: u32::try_from(row.file_count)?,
            segment_count: u32::try_from(row.segment_count)?,
            total_bytes: ByteCount::new(u64::try_from(row.total_bytes)?)
                .map_err(anyhow::Error::msg)?,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            priority: row
                .priority
                .map(|value| rd_core::DownloadPriority::from_i32(value as i32)),
            import_mode: serde_json::from_str(&format!("\"{}\"", row.import_mode))?,
            source_path: row.source_path,
            error: row.last_error,
            duplicate: false,
            has_password: row.has_password != 0,
            password: row.password,
            position: row.position,
            created_at: row.created_at,
        })
    }
}

fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}

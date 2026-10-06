//! The files and segments of an NZB import: the per-file listing and segment state writes.

use anyhow::Result;
use rd_core::{
    ByteCount, EventEnvelope, EventKind, NzbFileId, NzbFileStatus, NzbImportId, NzbSegmentId,
    NzbSegmentState, NzbSegmentStatus,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{enum_string, error::StoreError, parse_id, writer::insert_event};

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

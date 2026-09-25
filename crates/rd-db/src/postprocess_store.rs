use anyhow::Result;
use chrono::Utc;
use rd_core::{
    EventEnvelope, EventKind, NzbFileId, PostprocessKind, PostprocessState, PostprocessStep,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, writer::insert_event};

pub(crate) enum NzbCheckpoint {
    FileOutput {
        id: NzbFileId,
        output_path: String,
    },
    AssemblySegment {
        file_id: NzbFileId,
        segment_id: rd_core::NzbSegmentId,
        name: String,
        declared_size: u64,
        part_begin: u64,
        part_end: u64,
        crc32: u32,
    },
    Postprocess {
        owner_id: String,
        kind: PostprocessKind,
        source_path: String,
        state: PostprocessState,
        output_path: Option<String>,
        message: Option<String>,
        /// Stable code for this outcome, translated by the interface; `None` leaves the
        /// English `message` as the only thing that can be shown.
        code: Option<String>,
        /// Parameters interpolated into the translated code.
        params: rd_core::MessageParams,
        /// A plugin step's own bookkeeping, stored verbatim so an interrupted step resumes.
        /// `None` clears it, which is what finishing or failing a step should do.
        checkpoint: Option<Vec<u8>>,
    },
    /// Pre-registers the ordered pipeline of one owner as `queued` rows.
    EnqueueSteps {
        owner_id: String,
        steps: Vec<(PostprocessKind, String, i64)>,
    },
    /// Live progress of a running step, mirrored onto the owning package.
    Progress {
        owner_id: String,
        kind: PostprocessKind,
        source_path: String,
        stage: rd_core::PostprocessStage,
        percent: Option<u8>,
        current: Option<String>,
    },
}

pub(crate) async fn apply(
    connection: &mut SqliteConnection,
    checkpoint: NzbCheckpoint,
) -> Result<EventEnvelope> {
    let event = match &checkpoint {
        NzbCheckpoint::Progress {
            owner_id,
            kind,
            source_path,
            stage,
            percent,
            current,
        } => EventEnvelope::new(
            EventKind::PostprocessProgress,
            serde_json::json!({
                "owner_id": owner_id,
                "kind": kind,
                "source_path": source_path,
                "stage": stage,
                "percent": percent,
                "current": current,
            }),
        ),
        NzbCheckpoint::Postprocess {
            owner_id,
            kind,
            source_path,
            state,
            ..
        } => EventEnvelope::new(
            EventKind::PostprocessProgress,
            serde_json::json!({
                "owner_id": owner_id,
                "kind": kind,
                "source_path": source_path,
                "state": state,
            }),
        ),
        _ => EventEnvelope::new(
            EventKind::UsenetChanged,
            serde_json::json!({
                "resource": "nzb_checkpoint"
            }),
        ),
    };
    let mut tx = connection.begin().await?;
    match checkpoint {
        NzbCheckpoint::FileOutput { id, output_path } => {
            let result = sqlx::query("UPDATE nzb_files SET output_path = ? WHERE id = ?")
                .bind(output_path)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            anyhow::ensure!(
                result.rows_affected() == 1,
                StoreError::not_found("NZB file not found")
            );
        }
        NzbCheckpoint::AssemblySegment {
            file_id,
            segment_id,
            name,
            declared_size,
            part_begin,
            part_end,
            crc32,
        } => {
            let file = sqlx::query(
                "UPDATE nzb_files SET assembly_name = ?, declared_size = ? WHERE id = ?",
            )
            .bind(name)
            .bind(i64::try_from(declared_size)?)
            .bind(file_id.to_string())
            .execute(&mut *tx)
            .await?;
            anyhow::ensure!(
                file.rows_affected() == 1,
                StoreError::not_found("NZB file not found")
            );
            let segment = sqlx::query(
                "UPDATE nzb_segments SET state = 'completed', crc32 = ?, \
                 part_begin = ?, part_end = ? WHERE id = ? AND file_id = ?",
            )
            .bind(i64::from(crc32))
            .bind(i64::try_from(part_begin)?)
            .bind(i64::try_from(part_end)?)
            .bind(segment_id.to_string())
            .bind(file_id.to_string())
            .execute(&mut *tx)
            .await?;
            anyhow::ensure!(
                segment.rows_affected() == 1,
                StoreError::not_found("NZB segment not found")
            );
            // Unified queue: the linked download row mirrors segment progress.
            sqlx::query(
                "UPDATE downloads SET committed_bytes = (SELECT COALESCE(SUM(bytes), 0) FROM nzb_segments \
                 WHERE file_id = ? AND state = 'completed'), total_bytes = ?, updated_at = ? \
                 WHERE nzb_file_id = ?",
            )
            .bind(file_id.to_string())
            .bind(i64::try_from(declared_size)?)
            .bind(event.occurred_at)
            .bind(file_id.to_string())
            .execute(&mut *tx)
            .await?;
        }
        NzbCheckpoint::Postprocess {
            owner_id,
            kind,
            source_path,
            state,
            output_path,
            message,
            code,
            params,
            checkpoint,
        } => {
            let running = state == PostprocessState::Running;
            // An empty parameter map is stored as NULL rather than as `{}`: the two mean the
            // same thing and only one of them has to be read back.
            let params_json = if params.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&params)?)
            };
            sqlx::query(
                "INSERT INTO postprocess_steps \
                 (owner_id, kind, source_path, state, output_path, message, code, params_json, \
                  updated_at, position, progress_percent, started_at, checkpoint) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, NULL, CASE WHEN ? THEN ? ELSE NULL END, ?) \
                 ON CONFLICT(owner_id, kind, source_path) DO UPDATE SET \
                 state = excluded.state, output_path = excluded.output_path, \
                 message = excluded.message, code = excluded.code, \
                 params_json = excluded.params_json, updated_at = excluded.updated_at, \
                 progress_percent = CASE WHEN excluded.state = 'running' THEN progress_percent ELSE NULL END, \
                 started_at = CASE WHEN excluded.state = 'running' THEN COALESCE(started_at, excluded.started_at) ELSE started_at END, \
                 checkpoint = excluded.checkpoint",
            )
            .bind(owner_id)
            .bind(enum_string(kind)?)
            .bind(source_path)
            .bind(enum_string(state)?)
            .bind(output_path)
            .bind(message)
            .bind(code)
            .bind(params_json)
            .bind(event.occurred_at)
            .bind(running)
            .bind(event.occurred_at)
            .bind(checkpoint)
            .execute(&mut *tx)
            .await?;
        }
        NzbCheckpoint::EnqueueSteps { owner_id, steps } => {
            for (kind, source_path, position) in steps {
                sqlx::query(
                    "INSERT INTO postprocess_steps \
                     (owner_id, kind, source_path, state, updated_at, position) \
                     VALUES (?, ?, ?, 'queued', ?, ?) \
                     ON CONFLICT(owner_id, kind, source_path) DO UPDATE SET \
                     position = excluded.position, \
                     state = CASE WHEN state = 'running' THEN 'queued' ELSE state END, \
                     updated_at = excluded.updated_at",
                )
                .bind(&owner_id)
                .bind(enum_string(kind)?)
                .bind(source_path)
                .bind(event.occurred_at)
                .bind(position)
                .execute(&mut *tx)
                .await?;
            }
        }
        NzbCheckpoint::Progress {
            owner_id,
            kind,
            source_path,
            stage,
            percent,
            current,
        } => {
            sqlx::query(
                "UPDATE postprocess_steps SET progress_percent = ?, updated_at = ? \
                 WHERE owner_id = ? AND kind = ? AND source_path = ?",
            )
            .bind(percent.map(i64::from))
            .bind(event.occurred_at)
            .bind(&owner_id)
            .bind(enum_string(kind)?)
            .bind(&source_path)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE packages SET postprocess_stage = ?, postprocess_percent = ?, \
                 postprocess_current = ?, updated_at = ? WHERE id = ?",
            )
            .bind(stage.to_string())
            .bind(percent.map(i64::from))
            .bind(current)
            .bind(event.occurred_at)
            .bind(&owner_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn list(pool: &SqlitePool, owner_id: &str) -> Result<Vec<PostprocessStep>> {
    sqlx::query_as::<_, StepRow>(
        "SELECT owner_id, kind, source_path, state, output_path, message, code, params_json, \
         updated_at, position, progress_percent, started_at, checkpoint \
         FROM postprocess_steps WHERE owner_id = ? ORDER BY position, updated_at, source_path",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

#[derive(FromRow)]
struct StepRow {
    owner_id: String,
    kind: String,
    source_path: String,
    state: String,
    output_path: Option<String>,
    message: Option<String>,
    code: Option<String>,
    params_json: Option<String>,
    updated_at: chrono::DateTime<Utc>,
    position: i64,
    progress_percent: Option<i64>,
    started_at: Option<chrono::DateTime<Utc>>,
    checkpoint: Option<Vec<u8>>,
}

impl TryFrom<StepRow> for PostprocessStep {
    type Error = anyhow::Error;

    fn try_from(row: StepRow) -> Result<Self> {
        Ok(Self {
            owner_id: row.owner_id,
            kind: parse_enum(&row.kind)?,
            source_path: row.source_path,
            state: parse_enum(&row.state)?,
            output_path: row.output_path,
            message: row.message,
            code: row.code,
            // A blob nobody can read is not worth failing a queue listing over: the English
            // message is still there, and the code without its parameters still says what
            // happened.
            params: row
                .params_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default(),
            updated_at: row.updated_at,
            position: row.position,
            progress_percent: row
                .progress_percent
                .and_then(|value| u8::try_from(value).ok()),
            started_at: row.started_at,
            checkpoint: row.checkpoint,
        })
    }
}

fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    Ok(serde_json::from_str(&format!("\"{value}\""))?)
}

/// Removes every checkpoint of one owner (package or import).
pub(crate) async fn delete_for_owner(
    connection: &mut SqliteConnection,
    owner_id: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM postprocess_steps WHERE owner_id = ?")
        .bind(owner_id)
        .execute(connection)
        .await?;
    Ok(())
}

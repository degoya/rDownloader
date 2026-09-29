//! Uploading an archive to restore from (RD-160-03).
//!
//! An archive is as large as the installation's database, far past what one request may carry
//! (`container_upload::BODY_LIMIT_BYTES`), so it arrives in chunks: an upload is created, each
//! chunk names the offset it starts at, and a chunk that does not start where the file ends is
//! refused rather than written, so a repeated or reordered chunk cannot corrupt it. Uploads lie
//! in `restore-uploads/` below the data directory until they are removed or the next start
//! empties the folder.

use std::path::PathBuf;

use axum::{
    Json,
    body::Bytes,
    extract::{Path as UrlPath, Query, State},
    http::StatusCode,
};
use tokio::io::AsyncWriteExt;

use crate::restore_dto::{RestoreUploadChunkQuery, RestoreUploadResponse};
use crate::restore_service::layout;
use crate::{ApiError, AppState};

/// The largest chunk one request carries.
pub const CHUNK_LIMIT: u64 = 32 << 20;
/// The largest archive an upload accepts.
pub const MAX_UPLOAD_BYTES: u64 = 1 << 40;
/// How many uploads may lie waiting at once.
pub const MAX_UPLOADS: usize = 4;

/// Chunks are appended one at a time, whichever upload they belong to.
static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The file an upload id names; the id must be one this service made.
pub(crate) fn upload_path(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    let id = uuid::Uuid::parse_str(id)
        .map_err(|_| ApiError::not_found("backup.restore_upload_unknown", "No such upload"))?;
    Ok(layout(state)
        .uploads()
        .join(format!("{id}.{}", rd_backup::ARCHIVE_EXTENSION)))
}

async fn size_of(path: &std::path::Path) -> Result<u64, ApiError> {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.len())
        .map_err(|_| ApiError::not_found("backup.restore_upload_unknown", "No such upload"))
}

#[utoipa::path(
    post,
    path = "/api/v1/backups/restore/uploads",
    tag = "system",
    responses((status = 201, body = RestoreUploadResponse), (status = 409, body = crate::error::ErrorBody))
)]
pub async fn create_restore_upload(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<RestoreUploadResponse>), ApiError> {
    let folder = layout(&state).uploads();
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(anyhow::Error::from)?;
    let mut waiting = 0;
    let mut entries = tokio::fs::read_dir(&folder)
        .await
        .map_err(anyhow::Error::from)?;
    while let Some(entry) = entries.next_entry().await.map_err(anyhow::Error::from)? {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == rd_backup::ARCHIVE_EXTENSION)
        {
            waiting += 1;
        }
    }
    if waiting >= MAX_UPLOADS {
        return Err(ApiError::conflict(
            "backup.restore_uploads_full",
            "Too many uploads are waiting; remove one first",
        )
        .with_param("max", MAX_UPLOADS));
    }
    let id = uuid::Uuid::now_v7().to_string();
    let path = upload_path(&state, &id)?;
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(anyhow::Error::from)?;
    Ok((
        StatusCode::CREATED,
        Json(RestoreUploadResponse {
            id,
            size: 0,
            chunk_limit: CHUNK_LIMIT,
        }),
    ))
}

/// Refuses a chunk past [`CHUNK_LIMIT`]; the request body limit in front of it is wider.
fn refuse_oversized_chunk(length: u64) -> Result<(), ApiError> {
    if length > CHUNK_LIMIT {
        return Err(ApiError::payload_too_large(
            "backup.restore_chunk_too_large",
            "The chunk is larger than one request may carry",
        )
        .with_param("max", CHUNK_LIMIT));
    }
    Ok(())
}

#[utoipa::path(
    put,
    path = "/api/v1/backups/restore/uploads/{id}",
    tag = "system",
    params(("id" = String, Path, description = "Upload id"), RestoreUploadChunkQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, body = RestoreUploadResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody)
    )
)]
pub async fn append_restore_upload(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<RestoreUploadChunkQuery>,
    bytes: Bytes,
) -> Result<Json<RestoreUploadResponse>, ApiError> {
    let path = upload_path(&state, &id)?;
    let length = bytes.len() as u64;
    refuse_oversized_chunk(length)?;
    let _writing = WRITING.lock().await;
    let size = size_of(&path).await?;
    if query.offset != size {
        return Err(ApiError::conflict(
            "backup.restore_upload_offset",
            "The chunk does not start where the upload ends",
        )
        .with_param("expected", size));
    }
    if size.saturating_add(length) > MAX_UPLOAD_BYTES {
        return Err(ApiError::payload_too_large(
            "backup.restore_upload_too_large",
            "The archive is larger than an upload accepts",
        )
        .with_param("max", MAX_UPLOAD_BYTES));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .map_err(anyhow::Error::from)?;
    file.write_all(&bytes).await.map_err(anyhow::Error::from)?;
    file.flush().await.map_err(anyhow::Error::from)?;
    Ok(Json(RestoreUploadResponse {
        id,
        size: size + length,
        chunk_limit: CHUNK_LIMIT,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/backups/restore/uploads/{id}",
    tag = "system",
    params(("id" = String, Path, description = "Upload id")),
    responses((status = 204), (status = 404, body = crate::error::ErrorBody))
)]
pub async fn delete_restore_upload(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    let path = upload_path(&state, &id)?;
    let _writing = WRITING.lock().await;
    size_of(&path).await?;
    tokio::fs::remove_file(&path)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::{CHUNK_LIMIT, refuse_oversized_chunk};

    // A chunk at the limit has to get past the request body limit to reach the check at all.
    const _: () = assert!(CHUNK_LIMIT < rd_api_core::container_upload::BODY_LIMIT_BYTES as u64);

    #[test]
    fn a_chunk_past_the_limit_is_refused_as_too_large() {
        assert!(refuse_oversized_chunk(CHUNK_LIMIT).is_ok());
        let refused = refuse_oversized_chunk(CHUNK_LIMIT + 1).expect_err("past the limit");
        assert_eq!(refused.code(), "backup.restore_chunk_too_large");
        assert_eq!(
            refused.into_response().status(),
            axum::http::StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}

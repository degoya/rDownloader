//! NZB imports: the upload, the capture agent's drop and the review list.

use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
};
use rd_api_core::list_bounds::paged;
use rd_db::StoreErrorKind;

use crate::{
    ApiError, AppState,
    dto::{MessageResponse, NzbImportUpdateRequest, PageQuery},
};

/// The NZB imports waiting for review; `limit`/`offset` cut a page out of the list's order
/// (API-15).
#[utoipa::path(get, path = "/api/v1/nzb/imports", tag = "collector", params(PageQuery), responses((status = 200, body = [rd_core::NzbImport], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_nzb_imports(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<rd_core::NzbImport>>), ApiError> {
    let window = page.window()?;
    Ok(paged(window, state.database.list_nzb_imports().await?))
}

#[utoipa::path(patch, path = "/api/v1/nzb/imports/{id}", tag = "collector", params(("id" = rd_core::NzbImportId, Path)), request_body = NzbImportUpdateRequest, responses((status = 200, body = rd_core::NzbImport), (status = 400), (status = 404), (status = 409)))]
pub async fn update_nzb_import(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
    Json(request): Json<NzbImportUpdateRequest>,
) -> Result<Json<rd_core::NzbImport>, ApiError> {
    if let Some(category_id) = request.category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == category_id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    let category_id = if request.clear_category {
        Some(None)
    } else {
        request.category_id.map(Some)
    };
    if category_id.is_none() && request.priority.is_none() {
        return Err(ApiError::bad_request(
            "nzb.no_change",
            "No NZB import change specified",
        ));
    }
    state
        .database
        .update_nzb_import(
            id,
            rd_db::NzbImportChange {
                category_id,
                priority: request.priority,
            },
        )
        .await
        .map(Json)
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => {
                ApiError::not_found("nzb.import_not_found", "NZB import not found")
            }
            Some(StoreErrorKind::WrongState) => ApiError::conflict(
                "nzb.import_active",
                "Enqueued NZB imports must be changed in the download list",
            ),
            _ => error.into(),
        })
}

#[utoipa::path(delete, path = "/api/v1/nzb/imports/{id}", tag = "collector", params(("id" = rd_core::NzbImportId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_nzb_import(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_nzb_import(id)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => {
                ApiError::not_found("nzb.import_not_found", "NZB import not found")
            }
            Some(StoreErrorKind::WrongState) => {
                ApiError::conflict("nzb.import_active", "Active NZB imports cannot be deleted")
            }
            _ => error.into(),
        })?;
    Ok(Json(MessageResponse::new(
        "nzb.import_removed",
        "NZB import removed from the LinkGrabber",
    )))
}

#[utoipa::path(post, path = "/api/v1/nzb/imports", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (crate::container_upload::ContainerUpload = "application/json"))), responses((status = 201, body = rd_core::NzbImport), (status = 400, description = "The NZB cannot be parsed, a field is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit")))]
pub async fn import_nzb(
    State(state): State<AppState>,
    body: crate::container_upload::UploadBody,
) -> Result<(StatusCode, Json<rd_core::NzbImport>), ApiError> {
    let upload = body.read().await?;
    let category_id = match upload.category_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) => Some(id.parse::<rd_core::CategoryId>().map_err(|_| {
            ApiError::bad_request("nzb.category_invalid", "Category id is not valid")
        })?),
    };
    let priority = match upload.priority.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) => Some(
            serde_json::from_value::<rd_core::DownloadPriority>(serde_json::Value::String(
                text.to_owned(),
            ))
            .map_err(|_| {
                ApiError::bad_request(
                    "nzb.priority_invalid",
                    "Priority must be low, normal or high",
                )
            })?,
        ),
    };
    let file = upload.file.ok_or_else(|| {
        ApiError::bad_request(
            "nzb.file_field_missing",
            "Multipart field 'file' is missing",
        )
    })?;
    if file.bytes.len() > rd_collector::MAX_NZB_BYTES {
        return Err(
            ApiError::bad_request("nzb.too_large", "NZB exceeds the 64 MiB limit")
                .with_param("max_bytes", rd_collector::MAX_NZB_BYTES),
        );
    }
    // A supplied name wins over the uploaded file's name.
    let file_name = match upload.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => rd_files::sanitize_file_name(name),
        _ => file
            .file_name
            .as_deref()
            .map_or_else(|| "import.nzb".to_owned(), rd_files::sanitize_file_name),
    };
    let content = file.bytes;
    store_nzb_import(
        &state,
        &content,
        &file_name,
        category_id,
        // Somebody uploaded this file through the interface; a rule can target that the same
        // way it targets a hotfolder drop.
        rd_core::IngressSource::Manual,
        priority,
    )
    .await
    .map(|import| (StatusCode::CREATED, Json(import)))
}

/// Parses an NZB and records it as a review-mode import.
///
/// Split out of [`import_nzb`] so the SABnzbd adapter reaches the same parser, the same
/// password convention, the same validation and the same category selection instead of
/// assembling its own import rows.
pub async fn store_nzb_import(
    state: &AppState,
    content: &[u8],
    file_name: &str,
    category_id: Option<rd_core::CategoryId>,
    source: rd_core::IngressSource,
    priority: Option<rd_core::DownloadPriority>,
) -> Result<rd_core::NzbImport, ApiError> {
    let parsed = rd_collector::parse_nzb(content)
        .map_err(|error| ApiError::bad_request("nzb.parse_failed", error.to_string()))?;
    // SABnzbd convention: `release{{password}}.nzb` carries the archive password.
    let (file_name, marker_password) = rd_files::strip_password_marker(file_name);
    let password = marker_password.or_else(|| parsed.password.clone());
    let import = rd_db::NewNzbImport {
        name: rd_files::sanitize_file_name(&file_name),
        sha256: rd_api_core::input_checks::sha256_hex(content),
        category_id,
        source,
        priority,
        import_mode: rd_core::ImportMode::Review,
        source_path: None,
        password,
        // An upload is somebody handing a file over.
        announce_arrival: true,
        files: parsed
            .files
            .into_iter()
            .map(|file| rd_db::NewNzbFile {
                subject: file.subject,
                poster: file.poster,
                groups: file.groups,
                segments: file
                    .segments
                    .into_iter()
                    .map(|segment| rd_db::NewNzbSegment {
                        number: segment.number,
                        bytes: segment.bytes,
                        message_id: segment.message_id,
                    })
                    .collect(),
            })
            .collect(),
    };
    Ok(state.database.add_nzb_import(import).await?)
}

/// The capture agent's NZB drop: the same import, but multipart only — the agent's contract
/// did not change when the interface's route learned JSON (RD-120-31).
pub async fn capture_nzb(
    state: State<AppState>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<rd_core::NzbImport>), ApiError> {
    import_nzb(
        state,
        crate::container_upload::UploadBody::Multipart(multipart),
    )
    .await
}

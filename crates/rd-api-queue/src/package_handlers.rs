//! Package queue management: category, priority, manual order and LinkGrabber counterparts.

use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use rd_core::{CategoryId, DownloadPriority, DownloadState, PackageId};
use rd_db::{CategoryAssignment, PackageChange};

use crate::{
    ApiError, AppState,
    dto::{
        DownloadReorderRequest, MessageResponse, PackageBulkRequest, PackageDeleteRequest,
        PackageExtractRequest, PackageFolderRequest, PackageReorderRequest, PackageUpdateRequest,
        PageQuery,
    },
    package_clear::{blocking_code, busy_error},
};
use rd_api_core::list_bounds::{paged, validate_bulk};

mod removal;

pub use removal::*;

/// The one place a package name is checked, so the folder rename cannot drift from the label.
fn package_name(value: &str) -> Result<String, ApiError> {
    rd_api_core::input_checks::required_text(
        value,
        rd_api_core::input_checks::TextLimit::Chars(MAX_PACKAGE_NAME),
        "package.name_length",
        "Package name must be between 1 and 200 characters",
    )
}

const MAX_PACKAGE_NAME: usize = 200;

#[utoipa::path(patch, path = "/api/v1/packages/{id}", tag = "downloads", params(("id" = rd_core::PackageId, Path)), request_body = PackageUpdateRequest, responses((status = 200, body = rd_core::DownloadPackage), (status = 404), (status = 409)))]
pub async fn update_package(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
    Json(request): Json<PackageUpdateRequest>,
) -> Result<Json<rd_core::DownloadPackage>, ApiError> {
    let name = request.name.as_deref().map(package_name).transpose()?;
    let password = match (request.password, request.clear_password) {
        (_, true) => Some(None),
        (Some(value), false) => {
            let trimmed = value.trim();
            if trimmed.is_empty()
                || trimmed.chars().count() > 1024
                || trimmed.contains(['\n', '\r'])
            {
                return Err(ApiError::bad_request(
                    "package.password_length",
                    "Archive password must be between 1 and 1024 characters",
                )
                .with_param("max", 1024));
            }
            Some(Some(trimmed.to_owned()))
        }
        (None, false) => None,
    };
    let postprocess = crate::postprocess_handlers::postprocess_change(
        request.postprocess_level,
        request.clear_postprocess_level,
        request.script,
        request.clear_script,
    )?;
    let mut updated = apply_package_change(
        &state,
        vec![id],
        request.category_id,
        request.clear_category,
        request.priority,
        name,
        password,
        postprocess,
    )
    .await?;
    updated
        .pop()
        .map(Json)
        .ok_or_else(crate::error_codes::package_not_found)
}

#[utoipa::path(post, path = "/api/v1/packages/bulk", tag = "downloads", request_body = PackageBulkRequest, responses((status = 200, body = [rd_core::DownloadPackage]), (status = 409)))]
pub async fn bulk_update_packages(
    State(state): State<AppState>,
    Json(request): Json<PackageBulkRequest>,
) -> Result<Json<Vec<rd_core::DownloadPackage>>, ApiError> {
    validate_bulk(request.ids.len())?;
    let postprocess = crate::postprocess_handlers::postprocess_change(
        request.postprocess_level,
        request.clear_postprocess_level,
        request.script,
        request.clear_script,
    )?;
    Ok(Json(
        apply_package_change(
            &state,
            request.ids,
            request.category_id,
            request.clear_category,
            request.priority,
            None,
            None,
            postprocess,
        )
        .await?,
    ))
}

/// Renames a package **and the folder its files live in** (RD-106-13).
///
/// The plain `PATCH /api/v1/packages/{id}` deliberately does not do this: it changes a label,
/// and a label change must not move data. This does, and is therefore its own request, with
/// its own refusals.
///
/// Two-phase, like a category change: the row is written first — new name, new destination,
/// the old one kept in `previous_destination`, every stored absolute path rewritten — and the
/// scheduler then carries the folder over. An interruption between the two leaves the package
/// pointing at a folder that does not exist yet, which is precisely the state
/// `relocate_package` is built to finish, on the next pass or after a restart.
///
/// **A taken name is refused, not avoided.** Every other path in this codebase reaches for
/// `rd_files::collision_free_path`, which appends ` (1)` and moves on. That is right when a
/// file lands somewhere by itself and nobody is watching, and wrong here: somebody typed this
/// name, and silently storing a different one would leave them looking at a folder they did
/// not ask for and cannot tell apart from the one they meant.
#[utoipa::path(post, path = "/api/v1/packages/{id}/folder", tag = "downloads", params(("id" = rd_core::PackageId, Path)), request_body = PackageFolderRequest, responses((status = 200, body = rd_core::DownloadPackage), (status = 404), (status = 409)))]
pub async fn rename_package_folder(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
    Json(request): Json<PackageFolderRequest>,
) -> Result<Json<rd_core::DownloadPackage>, ApiError> {
    let name = package_name(&request.name)?;
    let package = state
        .database
        .get_package(id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    if package.destination.is_empty() {
        return Err(ApiError::conflict(
            "package.folder_unset",
            "This package has no folder of its own yet",
        ));
    }
    if package.state == rd_core::PackageState::Postprocessing {
        return Err(ApiError::conflict(
            "package.folder_busy",
            "Wait until the package is finished before renaming its folder",
        ));
    }
    // Out of scope by decision, not by accident: a running transfer holds an open handle in the
    // old folder, and moving the folder out from under it would cost the resume point.
    if state
        .database
        .downloads_for_package(id)
        .await?
        .iter()
        .any(|file| is_transferring(file.state))
    {
        return Err(ApiError::conflict(
            "package.folder_busy",
            "Wait until the package is finished before renaming its folder",
        ));
    }

    let current = std::path::Path::new(&package.destination);
    let target = rd_files::renamed_package_directory(current, &name).ok_or_else(|| {
        ApiError::conflict(
            "package.folder_unset",
            "This package has no folder of its own yet",
        )
    })?;
    let destination = target.to_string_lossy().into_owned();
    if destination != package.destination {
        // The one whole-table read left here: rd-db has no lookup by destination, and it is
        // only needed when no folder of that name exists on disk.
        let taken = tokio::fs::try_exists(&target).await.unwrap_or(true)
            || state
                .database
                .list_packages()
                .await?
                .iter()
                .any(|other| other.id != id && other.destination == destination);
        if taken {
            return Err(ApiError::conflict(
                "package.folder_exists",
                "A folder of that name is already there",
            )
            .with_param(
                "name",
                target.file_name().map_or_else(
                    || name.clone(),
                    |value| value.to_string_lossy().into_owned(),
                ),
            ));
        }
    }

    let updated = state
        .database
        .rename_package_directory(id, name, destination)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    // The row already names the new folder; carrying the data over is what makes it true. A
    // failure here is not lost — `previous_destination` keeps the move outstanding, and the
    // next completed file or restart runs it again.
    if let Err(error) = state.scheduler.relocate_package(id).await {
        tracing::warn!(package_id = %id, %error, "package folder was not renamed on disk");
    }
    Ok(Json(updated))
}

/// A file that is writing right now, and would lose its handle if the folder moved.
fn is_transferring(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting
            | DownloadState::Seeding
    )
}

#[utoipa::path(post, path = "/api/v1/packages/{id}/extract", tag = "downloads", params(("id" = rd_core::PackageId, Path)), responses((status = 202, body = MessageResponse), (status = 409)))]
pub async fn extract_package(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
) -> Result<(axum::http::StatusCode, Json<MessageResponse>), ApiError> {
    queue_extraction(&state, vec![id], rd_extract::ExtractionTrigger::Manual).await
}

/// Post-processes a package whose verification failed, once, anyway.
///
/// The one-off counterpart to the `safe_postproc` setting (RD-104-04): a broken PAR2 set
/// beside intact archives used to leave a package with no way forward at all, and the answer
/// to that should not be a global switch somebody then forgets to put back.
#[utoipa::path(post, path = "/api/v1/packages/{id}/extract/force", tag = "downloads", params(("id" = rd_core::PackageId, Path)), responses((status = 202, body = MessageResponse), (status = 409)))]
pub async fn force_extract_package(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
) -> Result<(axum::http::StatusCode, Json<MessageResponse>), ApiError> {
    queue_extraction(&state, vec![id], rd_extract::ExtractionTrigger::Force).await
}

#[utoipa::path(post, path = "/api/v1/packages/extract", tag = "downloads", request_body = PackageExtractRequest, responses((status = 202, body = MessageResponse), (status = 409)))]
pub async fn extract_packages(
    State(state): State<AppState>,
    Json(request): Json<PackageExtractRequest>,
) -> Result<(axum::http::StatusCode, Json<MessageResponse>), ApiError> {
    validate_bulk(request.ids.len())?;
    queue_extraction(&state, request.ids, rd_extract::ExtractionTrigger::Manual).await
}

#[utoipa::path(get, path = "/api/v1/packages/{id}/postprocess", tag = "downloads", params(("id" = rd_core::PackageId, Path)), responses((status = 200, body = [rd_core::PostprocessStep])))]
pub async fn list_package_postprocess(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
) -> Result<Json<Vec<rd_core::PostprocessStep>>, ApiError> {
    Ok(Json(
        state
            .database
            .list_postprocess_steps(&id.to_string())
            .await?,
    ))
}

async fn queue_extraction(
    state: &AppState,
    ids: Vec<PackageId>,
    trigger: rd_extract::ExtractionTrigger,
) -> Result<(axum::http::StatusCode, Json<MessageResponse>), ApiError> {
    let downloads = state.database.list_downloads().await?;
    let mut queued = 0_usize;
    for id in ids {
        let has_completed = downloads
            .iter()
            .any(|file| file.package_id == id && file.state == DownloadState::Completed);
        if !has_completed {
            continue;
        }
        state.extraction.request(id, trigger).await?;
        queued += 1;
    }
    if queued == 0 {
        return Err(ApiError::conflict(
            "package.nothing_to_extract",
            "None of the packages contains completed files to extract",
        ));
    }
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(
            MessageResponse::new(
                "package.extract_queued",
                format!("Extraction queued for {queued} package(s)"),
            )
            .with_count(queued),
        ),
    ))
}

#[utoipa::path(post, path = "/api/v1/packages/reorder", tag = "downloads", request_body = PackageReorderRequest, responses((status = 200, body = MessageResponse)))]
pub async fn reorder_packages(
    State(state): State<AppState>,
    Json(request): Json<PackageReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    state.database.reorder_packages(request.ids).await?;
    Ok(Json(MessageResponse::new(
        "package.order_saved",
        "Order saved",
    )))
}

/// Writes the manual file order inside one package.
///
/// The counterpart of `/api/v1/collector/candidates/reorder` for the download queue. Both take
/// the package's complete id list rather than a single move, and both refuse a list that is not
/// exactly that: positions are handed out 1..n from what arrives here, so a partial list would
/// renumber files the caller never saw.
#[utoipa::path(post, path = "/api/v1/downloads/reorder", tag = "downloads", request_body = DownloadReorderRequest, responses((status = 200, body = MessageResponse), (status = 400)))]
pub async fn reorder_downloads(
    State(state): State<AppState>,
    Json(request): Json<DownloadReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    let members: Vec<rd_core::DownloadId> = state
        .database
        .downloads_for_package(request.package_id)
        .await?
        .into_iter()
        .map(|download| download.id)
        .collect();
    crate::error_codes::validate_reorder(&members, &request.ids)?;
    state
        .database
        .reorder_downloads(request.package_id, request.ids)
        .await?;
    Ok(Json(MessageResponse::new(
        "download.order_saved",
        "File order saved",
    )))
}

#[allow(clippy::too_many_arguments)]
async fn apply_package_change(
    state: &AppState,
    ids: Vec<PackageId>,
    category_id: Option<CategoryId>,
    clear_category: bool,
    priority: Option<DownloadPriority>,
    name: Option<String>,
    password: Option<Option<String>>,
    postprocess: crate::postprocess_handlers::PostprocessChange,
) -> Result<Vec<rd_core::DownloadPackage>, ApiError> {
    let packages = state.database.list_packages().await?;
    let category = category_change(state, &ids, &packages, category_id, clear_category).await?;
    if category.is_none()
        && priority.is_none()
        && name.is_none()
        && password.is_none()
        && postprocess.level.is_none()
        && postprocess.script.is_none()
    {
        return Err(ApiError::bad_request(
            "package.no_change",
            "No change specified",
        ));
    }
    let moves_data = category.is_some();
    let change = PackageChange {
        category,
        priority,
        name,
        password,
        postprocess_level: postprocess.level,
        script: postprocess.script,
    };
    let updated = state.database.update_packages(ids.clone(), change).await?;
    if moves_data {
        // The row now points at the new folder; carry the data over to match it. Files that are
        // still transferring stay where they are and are picked up when they finish, so this
        // never has to refuse a package that happens to be busy.
        for id in &ids {
            if let Err(error) = state.scheduler.relocate_package(*id).await {
                tracing::warn!(
                    package_id = %id,
                    %error,
                    "package data was not moved to the new category directory"
                );
            }
        }
    }
    Ok(updated)
}

/// Resolves the requested category to the directory each package moves into; `None` = no change.
///
/// Every package keeps a folder of its own below the category directory — the same layout the
/// enqueue path builds — so a package that changed category is indistinguishable from one that
/// was created there.
async fn category_change(
    state: &AppState,
    ids: &[PackageId],
    packages: &[rd_core::DownloadPackage],
    category_id: Option<CategoryId>,
    clear_category: bool,
) -> Result<Option<CategoryAssignment>, ApiError> {
    if category_id.is_none() && !clear_category {
        return Ok(None);
    }
    if let Some(id) = category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    let root = crate::destination::download_destination(state, category_id)
        .await?
        .unwrap_or_else(|| state.scheduler.downloads_directory().to_path_buf());
    let destinations = ids
        .iter()
        .filter_map(|id| {
            let package = packages.iter().find(|package| package.id == *id)?;
            let directory = rd_files::package_directory(&root, &package.name);
            Some((*id, directory.to_string_lossy().into_owned()))
        })
        .collect();
    Ok(Some(CategoryAssignment {
        category_id,
        destinations,
    }))
}

/// Every package in the queue's order; `limit`/`offset` cut a page out of it (API-15).
#[utoipa::path(get, path = "/api/v1/packages", tag = "downloads", params(PageQuery), responses((status = 200, body = [rd_core::DownloadPackage], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_packages(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<rd_core::DownloadPackage>>), ApiError> {
    let window = page.window()?;
    Ok(paged(
        window,
        state.database.list_packages_with_passwords().await?,
    ))
}

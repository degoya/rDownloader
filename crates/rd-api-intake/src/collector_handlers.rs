//! LinkGrabber intake, packages, ordering and enqueueing.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use rd_core::{CandidateId, CollectorPackage, CollectorPackageId, LinkCandidateState};
use rd_db::{CollectorPackageChange, MoveTarget, StoreErrorKind};

use crate::{
    ApiError, AppState,
    dto::{
        CandidateCheckRequest, CandidateMoveRequest, CandidateRenameRequest,
        CandidateReorderRequest, CollectorIntakeRequest, CollectorIntakeResponse,
        CollectorPackageBulkRequest, CollectorPackageEnqueueRequest,
        CollectorPackageReorderRequest, CollectorPackageUpdateRequest, GrabberEntryReorderRequest,
        MessageResponse, PageQuery,
    },
};
use rd_api_core::list_bounds::{total_header, validate_bulk};

mod crawl;
mod intake;
mod links;
mod mirrors;

pub(crate) use intake::collector_intake_crawled;
pub use intake::collector_intake_inner;
pub use mirrors::*;

#[utoipa::path(post, path = "/api/v1/collector/batches", tag = "collector", request_body = CollectorIntakeRequest, responses((status = 201, body = CollectorIntakeResponse)))]
pub async fn collector_intake(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(mut request): Json<CollectorIntakeRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    request.source = crate::collector_source_sets::of_caller(&audit, request.source);
    let response = collector_intake_inner(&state, request).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(post, path = "/api/v1/capture/batches", tag = "capture", request_body = CollectorIntakeRequest, responses((status = 201, body = CollectorIntakeResponse), (status = 400, description = "Invalid links or request metadata"), (status = 401, description = "Capture token missing or revoked")))]
pub async fn capture_intake(
    state: State<AppState>,
    audit: crate::audit::AuditContext,
    Json(mut request): Json<CollectorIntakeRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    request.source = crate::collector_source_sets::captured(request.source);
    collector_intake(state, audit, Json(request)).await
}

/// Every LinkGrabber package in its list order; `limit`/`offset` cut a page out (API-15).
#[utoipa::path(get, path = "/api/v1/collector/packages", tag = "collector", params(PageQuery), responses((status = 200, body = [rd_core::CollectorPackage], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_collector_packages(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<CollectorPackage>>), ApiError> {
    let Some(window) = page.window()? else {
        return Ok((
            HeaderMap::new(),
            Json(state.database.list_collector_packages().await?),
        ));
    };
    let (offset, limit) = window.rows();
    let (packages, total) = state
        .database
        .collector_packages_page(offset, limit)
        .await?;
    Ok((total_header(Some(window), total), Json(packages)))
}

#[utoipa::path(patch, path = "/api/v1/collector/packages/{id}", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path)), request_body = CollectorPackageUpdateRequest, responses((status = 200, body = rd_core::CollectorPackage), (status = 404)))]
pub async fn update_collector_package(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
    Json(request): Json<CollectorPackageUpdateRequest>,
) -> Result<Json<CollectorPackage>, ApiError> {
    let change = package_change(
        &state,
        request.name,
        request.category_id,
        request.clear_category,
        request.priority,
        request.password,
        request.clear_password,
        crate::postprocess_handlers::postprocess_change(
            request.postprocess_level,
            request.clear_postprocess_level,
            request.script,
            request.clear_script,
        )?,
    )
    .await?;
    state
        .database
        .update_collector_packages(vec![id], change)
        .await?
        .pop()
        .map(Json)
        .ok_or_else(crate::error_codes::package_not_found)
}

#[utoipa::path(post, path = "/api/v1/collector/packages/bulk", tag = "collector", request_body = CollectorPackageBulkRequest, responses((status = 200, body = [rd_core::CollectorPackage])))]
pub async fn bulk_update_collector_packages(
    State(state): State<AppState>,
    Json(request): Json<CollectorPackageBulkRequest>,
) -> Result<Json<Vec<CollectorPackage>>, ApiError> {
    validate_bulk(request.ids.len())?;
    let change = package_change(
        &state,
        None,
        request.category_id,
        request.clear_category,
        request.priority,
        None,
        false,
        crate::postprocess_handlers::postprocess_change(
            request.postprocess_level,
            request.clear_postprocess_level,
            request.script,
            request.clear_script,
        )?,
    )
    .await?;
    Ok(Json(
        state
            .database
            .update_collector_packages(request.ids, change)
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/reorder", tag = "collector", request_body = CollectorPackageReorderRequest, responses((status = 200, body = MessageResponse)))]
pub async fn reorder_collector_packages(
    State(state): State<AppState>,
    Json(request): Json<CollectorPackageReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    state
        .database
        .reorder_collector_packages(request.ids)
        .await?;
    Ok(message("collector.order_saved", "Order saved"))
}

/// The manual order of the LinkGrabber list, both kinds in one sequence.
///
/// `POST /api/v1/collector/packages/reorder` can only number the collector packages, so an NZB
/// import sat wherever its creation time put it and could not be dragged at all. This endpoint
/// takes the mixed list and writes one sequence over both tables in a single transaction.
///
/// The body is a *slice* of that order: the entries that moved, and the `after` entry they were
/// dropped behind. Everything else keeps its relative order, so a drag costs the same two entries
/// whether the row sits at the top of the list or three thousand rows down. That is why the
/// ordinary bulk bound fits: it limits how many rows one gesture may move, not how deep into the
/// list the gesture reached. An earlier version had no anchor, so the list could only describe a
/// prefix and a move below entry 500 was refused for its depth alone.
#[utoipa::path(post, path = "/api/v1/collector/entries/reorder", tag = "collector", request_body = GrabberEntryReorderRequest, responses((status = 200, body = MessageResponse), (status = 400), (status = 404), (status = 409)))]
pub async fn reorder_grabber_entries(
    State(state): State<AppState>,
    Json(request): Json<GrabberEntryReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.entries.len())?;
    // The anchor is spliced *behind*, so it cannot also be one of the entries being moved: it
    // leaves the sequence together with them, and nothing is left to splice behind. The store
    // sees only an anchor it cannot find at that point; here both halves of the request are
    // still visible, so this is the one place the mistake can be named for what it is.
    if let Some(after) = request.after
        && request.entries.contains(&after)
    {
        return Err(ApiError::bad_request(
            "collector.entry_anchor_listed",
            "The anchor entry cannot be one of the entries being moved",
        ));
    }
    state
        .database
        .reorder_grabber_entries(request.entries, request.after)
        .await
        .map_err(|error| {
            crate::error_codes::store_error(
                &error,
                "collector.entry_not_found",
                "LinkGrabber entry not found",
                rd_db::StoreErrorKind::Duplicate,
                "collector.entry_duplicate",
                "LinkGrabber entry listed more than once",
            )
        })?;
    Ok(message("collector.order_saved", "Order saved"))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/regroup", tag = "collector", responses((status = 200, body = MessageResponse)))]
pub async fn regroup_collector_packages(
    State(state): State<AppState>,
) -> Result<Json<MessageResponse>, ApiError> {
    let batches: Vec<_> = state
        .database
        .list_collector_packages()
        .await?
        .into_iter()
        .map(|package| package.batch_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    state.database.regroup_collector_batches(batches).await?;
    Ok(message(
        "collector.packages_regrouped",
        "Packages regrouped",
    ))
}

#[utoipa::path(delete, path = "/api/v1/collector/packages/{id}", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_collector_package(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_collector_package(id)
        .await
        .map_err(|error| {
            crate::error_codes::store_error(
                &error,
                "package.not_found",
                "Package not found",
                StoreErrorKind::Busy,
                "collector.package_busy",
                "Package is being checked or enqueued",
            )
        })?;
    crate::torrent_intake::prune_checked_torrents(&state).await;
    Ok(message(
        "collector.package_removed",
        "Package removed from the LinkGrabber",
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/{id}/enqueue", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path)), responses((status = 201, body = rd_core::DownloadPackage), (status = 409)))]
pub async fn enqueue_collector_package(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
) -> Result<(StatusCode, Json<rd_core::DownloadPackage>), ApiError> {
    let outcome = crate::collector_enqueue::enqueue_package(&state, id, false, None).await?;
    Ok((StatusCode::CREATED, Json(outcome.package)))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/enqueue", tag = "collector", request_body = CollectorPackageEnqueueRequest, responses((status = 201, body = crate::dto::CollectorEnqueueBatchResponse)))]
pub async fn enqueue_collector_packages(
    State(state): State<AppState>,
    Json(request): Json<CollectorPackageEnqueueRequest>,
) -> Result<(StatusCode, Json<crate::dto::CollectorEnqueueBatchResponse>), ApiError> {
    validate_bulk(request.ids.len())?;
    let mut created = Vec::with_capacity(request.ids.len());
    let mut free_download_files = 0u32;
    let mut failed = 0u32;
    let mut first_error: Option<ApiError> = None;
    for id in request.ids {
        let only = request.candidate_ids.clone();
        match crate::collector_enqueue::enqueue_package(&state, id, request.paused, only).await {
            Ok(outcome) => {
                created.push(outcome.package);
                free_download_files += outcome.free_download_files;
            }
            Err(error) => {
                tracing::warn!(package_id = %id, message = error.message(), "enqueue failed");
                failed += 1;
                first_error.get_or_insert(error);
            }
        }
    }
    if created.is_empty()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(crate::dto::CollectorEnqueueBatchResponse {
            first_error: first_error.map(|error| error.message().to_owned()),
            created,
            failed,
            free_download_files,
        }),
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/move", tag = "collector", request_body = CandidateMoveRequest, responses((status = 200, body = rd_core::CollectorPackage)))]
pub async fn move_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateMoveRequest>,
) -> Result<Json<CollectorPackage>, ApiError> {
    validate_bulk(request.ids.len())?;
    let target = match (request.package_id, request.new_package_name) {
        (Some(id), _) => MoveTarget::Existing(id),
        (None, Some(name)) if !name.trim().is_empty() => MoveTarget::New { name },
        _ => {
            return Err(ApiError::bad_request(
                "collector.move_target_missing",
                "Target package or new package name is missing",
            ));
        }
    };
    Ok(Json(
        state.database.move_candidates(request.ids, target).await?,
    ))
}

/// Writes the manual link order inside one package.
///
/// The list has to be exactly the package's links, each of them once: the store hands out the
/// positions 1..n from it, and its `UPDATE` is fenced by `package_id`, so a foreign id used to
/// be a silent no-op that still answered "Order saved". `/api/v1/downloads/reorder` applies the
/// same rule to the download queue.
#[utoipa::path(post, path = "/api/v1/collector/candidates/reorder", tag = "collector", request_body = CandidateReorderRequest, responses((status = 200, body = MessageResponse), (status = 400)))]
pub async fn reorder_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    let members: Vec<rd_core::CandidateId> = state
        .database
        .list_candidates()
        .await?
        .into_iter()
        .filter(|candidate| candidate.package_id == Some(request.package_id))
        .map(|candidate| candidate.id)
        .collect();
    crate::error_codes::validate_reorder(&members, &request.ids)?;
    state
        .database
        .reorder_candidates(request.package_id, request.ids)
        .await?;
    Ok(message("collector.order_saved", "Order saved"))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/check", tag = "collector", request_body = CandidateCheckRequest, responses((status = 202, body = MessageResponse)))]
pub async fn check_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateCheckRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let ids = match request.ids {
        Some(ids) if !ids.is_empty() => ids,
        _ => state
            .database
            .list_candidates()
            .await?
            .into_iter()
            .filter(|candidate| {
                !matches!(
                    candidate.state,
                    LinkCandidateState::Checking | LinkCandidateState::Resolving
                )
            })
            .map(|candidate| candidate.id)
            .collect(),
    };
    let count = ids.len();
    state.link_check.check(ids).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            MessageResponse::new(
                "collector.check_started",
                format!("Check started for {count} link(s)"),
            )
            .with_count(count),
        ),
    ))
}

#[utoipa::path(patch, path = "/api/v1/collector/candidates/{id}", tag = "collector", params(("id" = rd_core::CandidateId, Path)), request_body = CandidateRenameRequest, responses((status = 200, body = rd_core::LinkCandidate), (status = 404)))]
pub async fn rename_candidate(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<CandidateRenameRequest>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    if let Some(variant) = request.media_variant.as_deref().map(str::trim) {
        let candidate = state
            .database
            .set_candidate_media_variant(id, variant.to_owned())
            .await
            .map_err(|error| match rd_db::store_kind(&error) {
                Some(StoreErrorKind::NotFound) => {
                    ApiError::not_found("collector.candidate_not_found", "Link not found")
                }
                Some(StoreErrorKind::UnknownMediaVariant) => {
                    ApiError::bad_request("media.variant_unknown", "Unknown media variant")
                }
                Some(StoreErrorKind::NoMediaMetadata) => {
                    ApiError::bad_request("media.not_media_link", "This link has no media variants")
                }
                _ => ApiError::conflict("collector.candidate_busy", "Link is busy"),
            })?;
        if request.file_name.is_none() {
            return Ok(Json(candidate));
        }
    }
    let Some(file_name) = request.file_name.as_deref() else {
        return Err(ApiError::bad_request(
            "package.no_change",
            "No change specified",
        ));
    };
    let file_name = rd_files::sanitize_file_name(file_name.trim());
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err(ApiError::bad_request(
            "collector.file_name_length",
            "File name must be between 1 and 255 characters",
        )
        .with_param("max", 255));
    }
    state
        .database
        .set_candidate_file_name(id, file_name)
        .await
        .map(Json)
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "collector.candidate_not_found",
                "Link not found",
            )
        })
}

#[allow(clippy::too_many_arguments)]
async fn package_change(
    state: &AppState,
    name: Option<String>,
    category_id: Option<rd_core::CategoryId>,
    clear_category: bool,
    priority: Option<rd_core::DownloadPriority>,
    password: Option<String>,
    clear_password: bool,
    postprocess: crate::postprocess_handlers::PostprocessChange,
) -> Result<CollectorPackageChange, ApiError> {
    let name = match name {
        Some(value) => Some(rd_api_core::input_checks::required_text(
            &value,
            rd_api_core::input_checks::TextLimit::Chars(200),
            "package.name_length",
            "Package name must be between 1 and 200 characters",
        )?),
        None => None,
    };
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
    let category = if clear_category {
        Some(None)
    } else {
        category_id.map(Some)
    };
    let password = match (password, clear_password) {
        (_, true) => Some(None),
        (Some(value), false) => Some(Some(rd_api_core::input_checks::required_text(
            &value,
            rd_api_core::input_checks::TextLimit::Chars(1024),
            "package.password_length",
            "Archive password must be between 1 and 1024 characters",
        )?)),
        (None, false) => None,
    };
    if name.is_none()
        && category.is_none()
        && priority.is_none()
        && password.is_none()
        && postprocess.level.is_none()
        && postprocess.script.is_none()
    {
        return Err(ApiError::bad_request(
            "package.no_change",
            "No change specified",
        ));
    }
    Ok(CollectorPackageChange {
        name,
        category_id: category,
        priority,
        password,
        postprocess_level: postprocess.level,
        script: postprocess.script,
    })
}

fn message(code: &str, text: &str) -> Json<MessageResponse> {
    Json(MessageResponse::new(code, text))
}

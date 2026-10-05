//! Removing packages from the queue, with their files or without them.

use super::*;

/// `?force=true` on a package removal: cancel whatever is still running and remove it anyway.
#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct PackageDeleteParams {
    #[serde(default)]
    pub force: bool,
}

#[utoipa::path(delete, path = "/api/v1/packages/{id}", tag = "downloads", params(("id" = rd_core::PackageId, Path), PackageDeleteParams), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_package(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<PackageId>,
    Query(params): Query<PackageDeleteParams>,
) -> Result<Json<MessageResponse>, ApiError> {
    let removed = remove_packages(&state, vec![id], params.force).await?;
    if removed == 0 {
        return Err(crate::error_codes::package_not_found());
    }
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PackageDeleted)
            .by(&audit)
            .target("package", id)
            .detail("forced", params.force),
    )
    .await;
    Ok(Json(MessageResponse::new(
        "package.removed",
        "Package removed from the download list",
    )))
}

#[utoipa::path(post, path = "/api/v1/packages/delete", tag = "downloads", request_body = PackageDeleteRequest, responses((status = 200, body = MessageResponse), (status = 409)))]
pub async fn delete_packages(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<PackageDeleteRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    validate_bulk(request.ids.len())?;
    let ids = request.ids.clone();
    let removed = remove_packages(&state, request.ids, request.force).await?;
    // One record per package rather than one per request: an audit log whose rows mean
    // "somebody deleted between one and two hundred things" cannot answer whether a
    // particular package was deleted, which is the only question anybody asks of it.
    for id in ids {
        crate::audit::record(
            &state,
            crate::audit::AuditEvent::success(rd_core::AuditAction::PackageDeleted)
                .by(&audit)
                .target("package", id)
                .detail("forced", request.force)
                .detail("bulk", true),
        )
        .await;
    }
    Ok(Json(
        MessageResponse::new(
            "package.bulk_removed",
            format!("{removed} package(s) removed from the download list"),
        )
        .with_count(removed),
    ))
}

/// Removes every file (and incomplete part file) of the packages; the package rows disappear
/// with their last file. Returns the number of packages found.
///
/// `force` decides what happens to a package that is still working. Without it the request is
/// refused, because removing such a package cancels its running files and deletes what they had
/// already written — the silent version of that cost a user the half-downloaded part of a
/// package he only meant to tidy up (RD-107-07). With it the caller has said so explicitly.
pub async fn remove_packages(
    state: &AppState,
    ids: Vec<PackageId>,
    force: bool,
) -> Result<usize, ApiError> {
    remove_packages_discarding(state, ids, force, false).await
}

/// [`remove_packages`], and with `discard_partial` every file that had not finished takes what
/// it wrote outside staging with it (RD-180-21). Finished and seeding files keep their data.
pub(crate) async fn remove_packages_discarding(
    state: &AppState,
    ids: Vec<PackageId>,
    force: bool,
    discard_partial: bool,
) -> Result<usize, ApiError> {
    let packages = state.database.list_packages().await?;
    let downloads = state.database.list_downloads().await?;
    // Only read when it can change the answer: an unconditional removal does not care.
    let pending = if force {
        HashSet::new()
    } else {
        state.extraction.pending().await
    };
    let mut removed = 0_usize;
    let mut errors = Vec::new();
    for id in ids {
        let Some(package) = packages.iter().find(|package| package.id == id) else {
            continue;
        };
        if !force && let Some(code) = blocking_code(package, &downloads, &pending) {
            return Err(busy_error(code, &package.name));
        }
        removed += 1;
        for file in downloads.iter().filter(|file| file.package_id == id) {
            let discard = discard_partial
                && !matches!(
                    file.state,
                    DownloadState::Completed | DownloadState::Seeding
                );
            if let Err(error) =
                crate::download_handlers::remove_with_cancel(state, file.id, discard).await
            {
                errors.push(format!("{}: {error}", file.file_name));
            }
        }
    }
    if let Some(first) = errors.first() {
        let count = errors.len();
        return Err(ApiError::conflict(
            "package.files_remove_failed",
            format!("{count} file(s) could not be removed: {first}"),
        )
        .with_param("count", count)
        .with_param("detail", first));
    }
    Ok(removed)
}

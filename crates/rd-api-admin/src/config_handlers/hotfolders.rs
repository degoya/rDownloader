//! Hot folders: list, create, change and delete.

use super::*;

#[utoipa::path(get, path = "/api/v1/hotfolders", tag = "configuration", responses((status = 200, body = [rd_core::HotFolderConfig])))]
pub async fn list_hotfolders(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::HotFolderConfig>>, ApiError> {
    Ok(Json(state.database.list_hotfolders().await?))
}

pub(super) async fn validated_hotfolder(
    state: &AppState,
    request: CreateHotFolderRequest,
) -> Result<rd_db::NewHotFolder, ApiError> {
    validate_name(&request.name)?;
    for subfolder in [&request.processed_path, &request.failed_path] {
        required_text(
            subfolder,
            TextLimit::Unbounded,
            "hotfolder.subfolders_required",
            "Processed and failed paths must not be empty",
        )?;
    }
    validate_relative(
        &request.processed_path,
        "hotfolder.processed_path_invalid",
        "Processed path",
    )?;
    validate_relative(
        &request.failed_path,
        "hotfolder.failed_path_invalid",
        "Failed path",
    )?;
    required_text(
        &request.path,
        TextLimit::Unbounded,
        "hotfolder.path_required",
        "Hotfolder path is required",
    )?;
    if matches!(request.executor, rd_core::HotFolderExecutor::Daemon) {
        let path = PathBuf::from(&request.path);
        if !path.is_absolute() {
            return Err(ApiError::bad_request(
                "hotfolder.path_not_absolute",
                "Daemon hotfolder path must be absolute",
            ));
        }
        // Before the folder is created: a refused path must not be left behind as a directory.
        refuse_protected_hotfolder(&path, &protected_directories(state, None).await)?;
        tokio::fs::create_dir_all(path)
            .await
            .map_err(anyhow::Error::new)?;
    }
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
    Ok(rd_db::NewHotFolder {
        name: request.name.trim().to_owned(),
        executor: request.executor,
        path: request.path,
        recursive: request.recursive,
        category_id: request.category_id,
        import_mode: request.import_mode,
        processed_path: request.processed_path,
        failed_path: request.failed_path,
        enabled: request.enabled,
    })
}

#[utoipa::path(post, path = "/api/v1/hotfolders", tag = "configuration", request_body = CreateHotFolderRequest, responses((status = 201, body = rd_core::HotFolderConfig)))]
pub async fn create_hotfolder(
    State(state): State<AppState>,
    Json(request): Json<CreateHotFolderRequest>,
) -> Result<(StatusCode, Json<rd_core::HotFolderConfig>), ApiError> {
    let input = validated_hotfolder(&state, request).await?;
    let value = state.database.create_hotfolder(input).await?;
    state.hotfolders.start(value.clone()).await?;
    Ok((StatusCode::CREATED, Json(value)))
}

#[utoipa::path(put, path = "/api/v1/hotfolders/{id}", tag = "configuration", params(("id" = rd_core::HotFolderId, Path)), request_body = CreateHotFolderRequest, responses((status = 200, body = rd_core::HotFolderConfig), (status = 404)))]
pub async fn update_hotfolder(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::HotFolderId>,
    Json(request): Json<CreateHotFolderRequest>,
) -> Result<Json<rd_core::HotFolderConfig>, ApiError> {
    let input = validated_hotfolder(&state, request).await?;
    let value = state
        .database
        .update_hotfolder(id, input)
        .await
        .map_err(|error| {
            store_error(
                &error,
                "hotfolder.not_found",
                "Hotfolder not found",
                StoreErrorKind::InUse,
                "hotfolder.in_use",
                "The hotfolder is still in use",
            )
        })?;
    // The watcher holds the old path and enabled flag, so it has to be recreated.
    state.hotfolders.stop(id).await;
    state.hotfolders.start(value.clone()).await?;
    Ok(Json(value))
}

#[utoipa::path(delete, path = "/api/v1/hotfolders/{id}", tag = "configuration", params(("id" = rd_core::HotFolderId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_hotfolder(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::HotFolderId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state.database.delete_hotfolder(id).await.map_err(|error| {
        store_error(
            &error,
            "hotfolder.not_found",
            "Hotfolder not found",
            StoreErrorKind::InUse,
            "hotfolder.in_use",
            "The hotfolder is still in use",
        )
    })?;
    state.hotfolders.stop(id).await;
    Ok(Json(crate::dto::MessageResponse::new(
        "hotfolder.deleted",
        "Hotfolder deleted",
    )))
}

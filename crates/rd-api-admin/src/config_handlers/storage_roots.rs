//! Storage roots: list, create, change and delete.

use super::*;

/// A root's name and its folder are each unique; a second one is a `409`, created or renamed.
const ROOT_TAKEN: &str = "storage_root.name_or_path_taken";
const ROOT_TAKEN_MESSAGE: &str = "A storage root with this name or folder already exists";

/// Attaches the runtime persistence verdict to a root.
///
/// One probe per request: reading the mount table costs a single small procfs read, and
/// caching it would go stale the moment somebody mounts a volume.
pub(super) fn with_persistence(
    probe: &rd_files::PersistenceProbe,
    root: rd_core::StorageRootConfig,
) -> crate::dto::StorageRootResponse {
    let persistence = probe.classify(std::path::Path::new(&root.path)).into();
    crate::dto::StorageRootResponse { root, persistence }
}

#[utoipa::path(get, path = "/api/v1/storage-roots", tag = "configuration", responses((status = 200, body = [crate::dto::StorageRootResponse])))]
pub async fn list_storage_roots(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::dto::StorageRootResponse>>, ApiError> {
    let probe = rd_files::PersistenceProbe::detect();
    Ok(Json(
        state
            .database
            .list_storage_roots()
            .await?
            .into_iter()
            .map(|root| with_persistence(&probe, root))
            .collect(),
    ))
}

/// Validates the request and materialises the directory the root points at.
pub(super) async fn validated_storage_root(
    state: &AppState,
    id: rd_core::StorageRootId,
    request: CreateStorageRootRequest,
) -> Result<rd_db::NewStorageRoot, ApiError> {
    validate_name(&request.name)?;
    let path = PathBuf::from(&request.path);
    // Before the directory is created, so a refused root leaves nothing behind; again after,
    // against the canonical path, for what only exists once it does (a symlink on the way).
    let protected = protected_directories(state, None).await;
    if path.is_absolute() {
        refuse_protected(&path, &protected)?;
    }
    // Asked before creating the root, and it answers more than "is this absolute": whether the
    // directory can be created where it is asked for, and whether anything can be written into
    // it. Every one of those used to surface as an untyped failure and reach the client as
    // "internal service error" — `/downloads` in the setup wizard being the reported case, where
    // the path is absolute, passes the only check there was, and then cannot be created because
    // the filesystem root belongs to another user.
    rd_files::ensure_usable(&path).await.map_err(|problem| {
        ApiError::bad_request(problem.code(), problem.message()).with_param("path", path.display())
    })?;
    let root = rd_files::StorageRoot::create(id, request.name.clone(), path)
        .await
        .map_err(ApiError::from)?;
    refuse_protected(root.path(), &protected)?;
    Ok(rd_db::NewStorageRoot {
        name: request.name.trim().to_owned(),
        path: root.path().to_string_lossy().into_owned(),
        is_default: request.is_default,
        minimum_free_bytes: request.minimum_free_bytes,
    })
}

#[utoipa::path(post, path = "/api/v1/storage-roots", tag = "configuration", request_body = CreateStorageRootRequest, responses((status = 201, body = crate::dto::StorageRootResponse), (status = 409)))]
pub async fn create_storage_root(
    State(state): State<AppState>,
    Json(request): Json<CreateStorageRootRequest>,
) -> Result<(StatusCode, Json<crate::dto::StorageRootResponse>), ApiError> {
    // One id for the directory that gets materialised and the row that records it.
    let id = rd_core::StorageRootId::new();
    let input = validated_storage_root(&state, id, request).await?;
    let value = state
        .database
        .create_storage_root(id, input)
        .await
        .map_err(|error| store_duplicate(&error, ROOT_TAKEN, ROOT_TAKEN_MESSAGE))?;
    state.scheduler.reload_capacity_config().await?;
    let probe = rd_files::PersistenceProbe::detect();
    Ok((StatusCode::CREATED, Json(with_persistence(&probe, value))))
}

#[utoipa::path(put, path = "/api/v1/storage-roots/{id}", tag = "configuration", params(("id" = rd_core::StorageRootId, Path)), request_body = CreateStorageRootRequest, responses((status = 200, body = crate::dto::StorageRootResponse), (status = 404), (status = 409)))]
pub async fn update_storage_root(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::StorageRootId>,
    Json(request): Json<CreateStorageRootRequest>,
) -> Result<Json<crate::dto::StorageRootResponse>, ApiError> {
    let input = validated_storage_root(&state, id, request).await?;
    let value = state
        .database
        .update_storage_root(id, input)
        .await
        .map_err(|error| {
            store_error(
                &error,
                "storage_root.not_found",
                "Storage root not found",
                StoreErrorKind::Duplicate,
                ROOT_TAKEN,
                ROOT_TAKEN_MESSAGE,
            )
        })?;
    state.scheduler.reload_capacity_config().await?;
    let probe = rd_files::PersistenceProbe::detect();
    Ok(Json(with_persistence(&probe, value)))
}

#[utoipa::path(delete, path = "/api/v1/storage-roots/{id}", tag = "configuration", params(("id" = rd_core::StorageRootId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_storage_root(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    AxumPath(id): AxumPath<rd_core::StorageRootId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    // Read before the delete, so the record can name what went. The *name* only: a storage
    // root's path is a place on somebody's disk and is not what an audit log is for.
    let name = state
        .database
        .list_storage_roots()
        .await
        .ok()
        .and_then(|roots| {
            roots
                .into_iter()
                .find(|root| root.id == id)
                .map(|root| root.name)
        });
    state
        .database
        .delete_storage_root(id)
        .await
        .map_err(|error| {
            store_error(
                &error,
                "storage_root.not_found",
                "Storage root not found",
                StoreErrorKind::InUse,
                "storage_root.in_use",
                "The storage root is still used by categories",
            )
        })?;
    // Same as create and update: the capacity supervisor still holds the removed root as a
    // limit target until it is told otherwise.
    state.scheduler.reload_capacity_config().await?;
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::StorageRootDeleted)
        .by(&audit)
        .target("storage_root", id);
    if let Some(name) = name {
        event = event.named(name);
    }
    crate::audit::record(&state, event).await;
    Ok(Json(crate::dto::MessageResponse::new(
        "storage_root.deleted",
        "Storage root deleted",
    )))
}

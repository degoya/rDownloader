//! Categories and category rules: list, create, change and delete.

use super::*;

/// A category's name is unique; a second one is a `409`, created or renamed.
const CATEGORY_TAKEN: &str = "category.name_taken";
const CATEGORY_TAKEN_MESSAGE: &str = "A category with this name already exists";

#[utoipa::path(get, path = "/api/v1/categories", tag = "configuration", responses((status = 200, body = [rd_core::Category])))]
pub async fn list_categories(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::Category>>, ApiError> {
    Ok(Json(state.database.list_categories().await?))
}

pub(crate) async fn validated_category(
    state: &AppState,
    request: CreateCategoryRequest,
) -> Result<rd_db::NewCategory, ApiError> {
    validate_name(&request.name)?;
    validate_relative(
        &request.relative_path,
        "category.path_invalid",
        "Category path",
    )?;
    if !valid_color(&request.color) {
        return Err(ApiError::bad_request(
            "category.color_invalid",
            "Category color must be in #RRGGBB format",
        ));
    }
    let root = state
        .database
        .list_storage_roots()
        .await?
        .into_iter()
        .find(|root| root.id == request.storage_root_id)
        .ok_or_else(|| ApiError::bad_request("storage_root.not_found", "Storage root not found"))?;
    let allowlist = rd_files::StorageRoot::create(root.id, root.name, PathBuf::from(root.path))
        .await
        .map_err(ApiError::from)?;
    allowlist
        .resolve(Path::new(&request.relative_path))
        .map_err(ApiError::from)?;
    Ok(rd_db::NewCategory {
        name: request.name.trim().to_owned(),
        color: request.color.to_ascii_uppercase(),
        storage_root_id: request.storage_root_id,
        relative_path: request.relative_path,
        is_default: request.is_default,
        postprocess_level: request.postprocess_level,
        script: crate::postprocess_handlers::validate_script_name(request.script)?,
        cleanup_extensions: request
            .cleanup_extensions
            .map(crate::postprocess_handlers::normalize_cleanup_extensions)
            .transpose()?,
        recursive_unpack: request.recursive_unpack,
        unpack_to_subfolder: request.unpack_to_subfolder,
        unwrap_package_folder: request.unwrap_package_folder,
        direct_unpack: request.direct_unpack,
        malware_scan: request.malware_scan,
        sfv_verify: request.sfv_verify,
        safe_postproc: request.safe_postproc,
        delete_par2: request.delete_par2,
        upload_enabled: request.upload_enabled,
        upload_remote: crate::dto::normalize_upload_remote(request.upload_remote)?,
    })
}

#[utoipa::path(post, path = "/api/v1/categories", tag = "configuration", request_body = CreateCategoryRequest, responses((status = 201, body = rd_core::Category), (status = 409)))]
pub async fn create_category(
    State(state): State<AppState>,
    Json(request): Json<CreateCategoryRequest>,
) -> Result<(StatusCode, Json<rd_core::Category>), ApiError> {
    let input = validated_category(&state, request).await?;
    let value = state
        .database
        .create_category(input)
        .await
        .map_err(|error| store_duplicate(&error, CATEGORY_TAKEN, CATEGORY_TAKEN_MESSAGE))?;
    Ok((StatusCode::CREATED, Json(value)))
}

#[utoipa::path(put, path = "/api/v1/categories/{id}", tag = "configuration", params(("id" = rd_core::CategoryId, Path)), request_body = CreateCategoryRequest, responses((status = 200, body = rd_core::Category), (status = 404), (status = 409)))]
pub async fn update_category(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::CategoryId>,
    Json(request): Json<CreateCategoryRequest>,
) -> Result<Json<rd_core::Category>, ApiError> {
    let input = validated_category(&state, request).await?;
    let value = state
        .database
        .update_category(id, input)
        .await
        .map_err(|error| {
            store_error(
                &error,
                "category.not_found",
                "Category not found",
                StoreErrorKind::Duplicate,
                CATEGORY_TAKEN,
                CATEGORY_TAKEN_MESSAGE,
            )
        })?;
    Ok(Json(value))
}

#[utoipa::path(delete, path = "/api/v1/categories/{id}", tag = "configuration", params(("id" = rd_core::CategoryId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_category(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    AxumPath(id): AxumPath<rd_core::CategoryId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    // Read before the delete, so the record can name what went rather than only its id.
    let name = state
        .database
        .list_categories()
        .await
        .ok()
        .and_then(|categories| {
            categories
                .into_iter()
                .find(|category| category.id == id)
                .map(|category| category.name)
        });
    state.database.delete_category(id).await.map_err(|error| {
        store_error(
            &error,
            "category.not_found",
            "Category not found",
            StoreErrorKind::InUse,
            "category.in_use",
            "The category is still used by unfinished packages",
        )
    })?;
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::CategoryDeleted)
        .by(&audit)
        .target("category", id);
    if let Some(name) = name {
        event = event.named(name);
    }
    crate::audit::record(&state, event).await;
    Ok(Json(crate::dto::MessageResponse::new(
        "category.deleted",
        "Category deleted",
    )))
}

#[utoipa::path(get, path = "/api/v1/category-rules", tag = "configuration", responses((status = 200, body = [rd_core::CategoryRule])))]
pub async fn list_category_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::CategoryRule>>, ApiError> {
    Ok(Json(state.database.list_category_rules().await?))
}

pub(crate) async fn validated_category_rule(
    state: &AppState,
    request: CreateCategoryRuleRequest,
) -> Result<rd_db::NewCategoryRule, ApiError> {
    validate_name(&request.name)?;
    if let Some(pattern) = request.name_regex.as_deref() {
        Regex::new(pattern).map_err(|_| {
            ApiError::bad_request(
                "category_rule.name_regex_invalid",
                "Name regex is not a valid regular expression",
            )
        })?;
    }
    if let Some(domain) = request.domain.as_deref()
        && (domain != domain.to_ascii_lowercase() || domain.contains(['/', ':']))
    {
        return Err(ApiError::bad_request(
            "category_rule.domain_invalid",
            "Rule domain must be lowercase and must not contain a scheme, port or path",
        ));
    }
    if !state
        .database
        .list_categories()
        .await?
        .iter()
        .any(|category| category.id == request.category_id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    Ok(rd_db::NewCategoryRule {
        name: request.name.trim().to_owned(),
        priority: request.priority,
        source: request.source,
        domain: request.domain,
        protocol: request.protocol.map(|value| value.to_ascii_lowercase()),
        extension: request
            .extension
            .map(|value| value.trim_start_matches('.').to_ascii_lowercase()),
        mime_type: request.mime_type,
        name_regex: request.name_regex,
        name_target: request.name_target,
        category_id: request.category_id,
        enabled: request.enabled,
    })
}

#[utoipa::path(post, path = "/api/v1/category-rules", tag = "configuration", request_body = CreateCategoryRuleRequest, responses((status = 201, body = rd_core::CategoryRule)))]
pub async fn create_category_rule(
    State(state): State<AppState>,
    Json(request): Json<CreateCategoryRuleRequest>,
) -> Result<(StatusCode, Json<rd_core::CategoryRule>), ApiError> {
    let input = validated_category_rule(&state, request).await?;
    let value = state.database.create_category_rule(input).await?;
    Ok((StatusCode::CREATED, Json(value)))
}

#[utoipa::path(put, path = "/api/v1/category-rules/{id}", tag = "configuration", params(("id" = rd_core::CategoryRuleId, Path)), request_body = CreateCategoryRuleRequest, responses((status = 200, body = rd_core::CategoryRule), (status = 404)))]
pub async fn update_category_rule(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::CategoryRuleId>,
    Json(request): Json<CreateCategoryRuleRequest>,
) -> Result<Json<rd_core::CategoryRule>, ApiError> {
    let input = validated_category_rule(&state, request).await?;
    let value = state
        .database
        .update_category_rule(id, input)
        .await
        .map_err(|error| {
            store_error(
                &error,
                "category_rule.not_found",
                "Category rule not found",
                StoreErrorKind::InUse,
                "category_rule.in_use",
                "The category rule is still in use",
            )
        })?;
    Ok(Json(value))
}

#[utoipa::path(delete, path = "/api/v1/category-rules/{id}", tag = "configuration", params(("id" = rd_core::CategoryRuleId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_category_rule(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::CategoryRuleId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state
        .database
        .delete_category_rule(id)
        .await
        .map_err(|error| {
            store_error(
                &error,
                "category_rule.not_found",
                "Category rule not found",
                StoreErrorKind::InUse,
                "category_rule.in_use",
                "The category rule is still in use",
            )
        })?;
    Ok(Json(crate::dto::MessageResponse::new(
        "category_rule.deleted",
        "Category rule deleted",
    )))
}

use std::path::{Component, Path, PathBuf};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::StatusCode,
};
use rd_db::StoreErrorKind;
use regex::Regex;

use crate::{
    ApiError, AppState,
    dto::{
        AccountTestResponse, CreateAccountRequest, CreateCategoryRequest,
        CreateCategoryRuleRequest, CreateHotFolderRequest, CreateProxyProfileRequest,
        CreateStorageRootRequest, UpdateAccountRequest,
    },
    error_codes::store_error,
};

#[utoipa::path(get, path = "/api/v1/accounts", tag = "configuration", responses((status = 200, body = [rd_core::Account])))]
pub async fn list_accounts(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::Account>>, ApiError> {
    Ok(Json(state.database.list_accounts().await?))
}

#[utoipa::path(put, path = "/api/v1/accounts/{id}", tag = "configuration", params(("id" = rd_core::AccountId, Path)), request_body = UpdateAccountRequest, responses((status = 200, body = rd_core::Account), (status = 404)))]
pub async fn update_account(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::AccountId>,
    Json(request): Json<UpdateAccountRequest>,
) -> Result<Json<rd_core::Account>, ApiError> {
    crate::hosters::forget(id);
    validate_name(&request.provider)?;
    validate_name(&request.label)?;
    // An account whose plugin is no longer installed has to stay editable. Since RD-101-13 a
    // provider exists only while its plugin does, so uninstalling one would otherwise freeze
    // every account that used it — it could not even be switched off, which is the one thing
    // somebody would want to do with it. The provider is therefore validated when it is still
    // known, and when the request changes it; an unchanged unknown one is left alone. Creating
    // an account for an unknown provider stays refused.
    let keeps_its_provider = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .any(|account| account.id == id && account.provider == request.provider);
    if rd_provider_registry::by_slug(&request.provider).is_some() || !keeps_its_provider {
        validate_provider(
            &request.provider,
            request.username.as_deref(),
            request.credential_mode,
        )?;
    }
    validate_secret_value(
        request.secret.as_deref(),
        16 * 1024,
        "account.secret_invalid",
        "Secret",
    )?;
    validate_secret_value(
        request.cookies.as_deref(),
        4 * 1024 * 1024,
        "account.cookies_invalid",
        "Cookies",
    )?;
    validate_proxy_selection(&state, request.proxy_profile_id).await?;
    let (old_secret_ref, old_cookie_ref) = state
        .database
        .account_secret_refs(id)
        .await?
        .ok_or_else(crate::error_codes::account_not_found)?;

    let replaces_secret = request.secret.is_some();
    let new_secret_ref = match request.secret {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if request.clear_secret => None,
        None => old_secret_ref.clone(),
    };
    let replaces_cookie = request.cookies.is_some();
    let new_cookie_ref = match request.cookies {
        Some(value) => match state.secrets.put_string(value).await {
            Ok(reference) => Some(reference),
            Err(error) => {
                if replaces_secret {
                    cleanup_secrets(&state.secrets, [new_secret_ref]).await;
                }
                return Err(error.into());
            }
        },
        None if request.clear_cookies => None,
        None => old_cookie_ref.clone(),
    };
    let stored_secret = replaces_secret.then(|| new_secret_ref.clone()).flatten();
    let stored_cookie = replaces_cookie.then(|| new_cookie_ref.clone()).flatten();
    let result = state
        .database
        .update_account(
            id,
            rd_db::UpdateAccount {
                provider: request.provider.trim().to_ascii_lowercase(),
                label: request.label.trim().to_owned(),
                username: normalized_optional(request.username),
                credential_mode: request.credential_mode,
                secret_ref: new_secret_ref.clone(),
                cookie_ref: new_cookie_ref.clone(),
                proxy_profile_id: request.proxy_profile_id,
                enabled: request.enabled,
            },
        )
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            cleanup_secrets(&state.secrets, [stored_secret, stored_cookie]).await;
            return Err(error.into());
        }
    };
    cleanup_replaced_secret(&state.secrets, old_secret_ref, &new_secret_ref).await;
    cleanup_replaced_secret(&state.secrets, old_cookie_ref, &new_cookie_ref).await;
    Ok(Json(value))
}

#[utoipa::path(post, path = "/api/v1/accounts/{id}/test", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = AccountTestResponse), (status = 404), (status = 502)))]
pub async fn test_account(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::AccountId>,
) -> Result<Json<AccountTestResponse>, ApiError> {
    let account = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .find(|account| account.id == id)
        .ok_or_else(crate::error_codes::account_not_found)?;
    if !account.enabled {
        return Err(ApiError::bad_request(
            "account.disabled",
            "Provider account is disabled",
        ));
    }
    let status = state
        .scheduler
        .check_account(id)
        .await
        .map_err(account_check_failed)?;
    Ok(Json(AccountTestResponse {
        valid: status.valid,
        premium: status.premium,
        label: status.label.into_iter().map(Into::into).collect(),
        traffic_left: status.traffic_left,
    }))
}

/// Failure codes an account check passes through as they are, rather than quoting them.
///
/// A check otherwise answers `account.check_failed` with the provider's English reason, which is
/// all a plugin's own wording can be. A failure the service coded itself and the interface
/// translates is worth more than that: `captcha.page_without_widget` says what to do in the
/// reader's language (RD-120-45), where the quoted reason could only say it in English.
const PASSED_THROUGH_CHECK_CODES: &[&str] = &["captcha.page_without_widget"];

fn account_check_failed(failure: rd_core::Failure) -> ApiError {
    if let Some(code) = failure.code.as_deref().and_then(|code| {
        PASSED_THROUGH_CHECK_CODES
            .iter()
            .find(|known| **known == code)
    }) {
        return failure.params.iter().fold(
            ApiError::bad_gateway(code, failure.message.clone()),
            |error, (key, value)| error.with_param(key, value),
        );
    }
    ApiError::bad_gateway(
        "account.check_failed",
        format!("Provider account check failed: {}", failure.message),
    )
    .with_param("reason", failure.message)
}

#[utoipa::path(delete, path = "/api/v1/accounts/{id}", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_account(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::AccountId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    let references = state.database.delete_account(id).await.map_err(|error| {
        store_error(
            &error,
            "account.not_found",
            "Provider account not found",
            StoreErrorKind::InUse,
            "account.in_use",
            "The provider account is still used by unfinished downloads",
        )
    })?;
    cleanup_secrets(&state.secrets, [references.0, references.1]).await;
    crate::hosters::forget(id);
    Ok(Json(crate::dto::MessageResponse::new(
        "account.deleted",
        "Provider account deleted",
    )))
}

#[utoipa::path(post, path = "/api/v1/accounts", tag = "configuration", request_body = CreateAccountRequest, responses((status = 201, body = rd_core::Account)))]
pub async fn create_account(
    State(state): State<AppState>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<rd_core::Account>), ApiError> {
    validate_name(&request.provider)?;
    validate_name(&request.label)?;
    validate_provider(
        &request.provider,
        request.username.as_deref(),
        request.credential_mode,
    )?;
    validate_secret_value(
        request.secret.as_deref(),
        16 * 1024,
        "account.secret_invalid",
        "Secret",
    )?;
    validate_secret_value(
        request.cookies.as_deref(),
        4 * 1024 * 1024,
        "account.cookies_invalid",
        "Cookies",
    )?;
    validate_proxy_selection(&state, request.proxy_profile_id).await?;
    let secret_ref = store_optional(&state.secrets, request.secret).await?;
    let cookie_ref = match store_optional(&state.secrets, request.cookies).await {
        Ok(reference) => reference,
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref]).await;
            return Err(error);
        }
    };
    let result = state
        .database
        .create_account(rd_db::NewAccount {
            provider: request.provider.trim().to_ascii_lowercase(),
            label: request.label.trim().to_owned(),
            username: normalized_optional(request.username),
            credential_mode: request.credential_mode,
            secret_ref: secret_ref.clone(),
            cookie_ref: cookie_ref.clone(),
            proxy_profile_id: request.proxy_profile_id,
            enabled: request.enabled,
        })
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref, cookie_ref]).await;
            return Err(error.into());
        }
    };
    Ok((StatusCode::CREATED, Json(value)))
}

#[utoipa::path(get, path = "/api/v1/proxy-profiles", tag = "configuration", responses((status = 200, body = [rd_core::ProxyProfile])))]
pub async fn list_proxy_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::ProxyProfile>>, ApiError> {
    Ok(Json(state.database.list_proxy_profiles().await?))
}

/// Validates the editable proxy fields shared by create and update.
///
/// The password is deliberately not part of the returned value, and neither is the rule that
/// a username needs one: create and update disagree about what "no password in the request"
/// means — an update keeps the stored one — so the pairing is checked against the *effective*
/// credential by each caller instead.
fn validated_proxy_profile(
    request: &CreateProxyProfileRequest,
) -> Result<(url::Url, Option<String>), ApiError> {
    validate_name(&request.name)?;
    validate_secret_value(
        request.password.as_deref(),
        16 * 1024,
        "proxy.password_invalid",
        "Proxy password",
    )?;
    let username = normalized_optional(request.username.clone());
    let endpoint = url::Url::parse(&request.endpoint).map_err(|_| {
        ApiError::bad_request(
            "proxy.endpoint_invalid",
            "Proxy endpoint is not a valid URL",
        )
    })?;
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err(ApiError::bad_request(
            "proxy.credentials_in_url",
            "Proxy credentials must not be part of the URL",
        ));
    }
    let valid_scheme = match request.kind {
        rd_core::ProxyKind::Http => endpoint.scheme() == "http",
        rd_core::ProxyKind::Https => endpoint.scheme() == "https",
        rd_core::ProxyKind::Socks5 => matches!(endpoint.scheme(), "socks5" | "socks5h"),
    };
    if !valid_scheme || endpoint.host_str().is_none() {
        return Err(ApiError::bad_request(
            "proxy.scheme_mismatch",
            "Proxy URL scheme does not match the profile type",
        ));
    }
    Ok((endpoint, username))
}

fn proxy_credentials_incomplete() -> ApiError {
    ApiError::bad_request(
        "proxy.password_requires_username",
        "Proxy username and password must be set together",
    )
}

#[utoipa::path(post, path = "/api/v1/proxy-profiles", tag = "configuration", request_body = CreateProxyProfileRequest, responses((status = 201, body = rd_core::ProxyProfile)))]
pub async fn create_proxy_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateProxyProfileRequest>,
) -> Result<(StatusCode, Json<rd_core::ProxyProfile>), ApiError> {
    let (endpoint, username) = validated_proxy_profile(&request)?;
    if username.is_some() != request.password.is_some() {
        return Err(proxy_credentials_incomplete());
    }
    let secret_ref = store_optional(&state.secrets, request.password).await?;
    let result = state
        .database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: request.name.trim().to_owned(),
            kind: request.kind,
            endpoint,
            username,
            secret_ref: secret_ref.clone(),
        })
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref]).await;
            return Err(error.into());
        }
    };
    Ok((StatusCode::CREATED, Json(value)))
}

#[utoipa::path(put, path = "/api/v1/proxy-profiles/{id}", tag = "configuration", params(("id" = rd_core::ProxyProfileId, Path)), request_body = CreateProxyProfileRequest, responses((status = 200, body = rd_core::ProxyProfile), (status = 404)))]
pub async fn update_proxy_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::ProxyProfileId>,
    Json(request): Json<CreateProxyProfileRequest>,
) -> Result<Json<rd_core::ProxyProfile>, ApiError> {
    let (endpoint, username) = validated_proxy_profile(&request)?;
    let existing = state
        .database
        .proxy_profile(id)
        .await?
        .ok_or_else(|| ApiError::not_found("proxy.not_found", "Proxy profile not found"))?;
    // An omitted password keeps the stored one, the same bargain `update_account` strikes:
    // an edit that only renames a profile must not silently drop its credential.
    let replaces_secret = request.password.is_some();
    let new_secret_ref = match request.password {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None => existing.secret_ref.clone(),
    };
    // Checked against the effective credential rather than the request: an edit that only
    // renames a profile carries no password, and the stored one still pairs with the username.
    if username.is_some() != new_secret_ref.is_some() {
        if replaces_secret {
            cleanup_secrets(&state.secrets, [new_secret_ref]).await;
        }
        return Err(proxy_credentials_incomplete());
    }
    let result = state
        .database
        .update_proxy_profile(
            id,
            rd_db::NewProxyProfile {
                name: request.name.trim().to_owned(),
                kind: request.kind,
                endpoint,
                username,
                secret_ref: new_secret_ref.clone(),
            },
        )
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            if replaces_secret {
                cleanup_secrets(&state.secrets, [new_secret_ref]).await;
            }
            return Err(store_error(
                &error,
                "proxy.not_found",
                "Proxy profile not found",
                StoreErrorKind::InUse,
                "proxy.in_use",
                "The proxy profile is still in use",
            ));
        }
    };
    cleanup_replaced_secret(&state.secrets, existing.secret_ref, &new_secret_ref).await;
    Ok(Json(value))
}

#[utoipa::path(delete, path = "/api/v1/proxy-profiles/{id}", tag = "configuration", params(("id" = rd_core::ProxyProfileId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_proxy_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::ProxyProfileId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    // The store refuses a profile an account, a Usenet server or an unfinished download still
    // points at. The global selection lives in the settings document instead, so it is checked
    // here rather than there.
    if crate::handlers::read_settings(&state)
        .await?
        .global_proxy_profile_id
        == Some(id)
    {
        return Err(ApiError::conflict(
            "proxy.in_use",
            "The proxy profile is still selected as the global proxy",
        ));
    }
    let secret_ref = state.database.delete_proxy_profile(id).await.map_err(|error| {
        store_error(
            &error,
            "proxy.not_found",
            "Proxy profile not found",
            StoreErrorKind::InUse,
            "proxy.in_use",
            "The proxy profile is still used by accounts, Usenet servers or unfinished downloads",
        )
    })?;
    cleanup_secrets(&state.secrets, [secret_ref]).await;
    Ok(Json(crate::dto::MessageResponse::new(
        "proxy.deleted",
        "Proxy profile deleted",
    )))
}

/// Attaches the runtime persistence verdict to a root.
///
/// One probe per request: reading the mount table costs a single small procfs read, and
/// caching it would go stale the moment somebody mounts a volume.
fn with_persistence(
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
async fn validated_storage_root(
    id: rd_core::StorageRootId,
    request: CreateStorageRootRequest,
) -> Result<rd_db::NewStorageRoot, ApiError> {
    validate_name(&request.name)?;
    let path = PathBuf::from(&request.path);
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
    Ok(rd_db::NewStorageRoot {
        name: request.name.trim().to_owned(),
        path: root.path().to_string_lossy().into_owned(),
        is_default: request.is_default,
        minimum_free_bytes: request.minimum_free_bytes,
    })
}

#[utoipa::path(post, path = "/api/v1/storage-roots", tag = "configuration", request_body = CreateStorageRootRequest, responses((status = 201, body = crate::dto::StorageRootResponse)))]
pub async fn create_storage_root(
    State(state): State<AppState>,
    Json(request): Json<CreateStorageRootRequest>,
) -> Result<(StatusCode, Json<crate::dto::StorageRootResponse>), ApiError> {
    // One id for the directory that gets materialised and the row that records it.
    let id = rd_core::StorageRootId::new();
    let input = validated_storage_root(id, request).await?;
    let value = state.database.create_storage_root(id, input).await?;
    state.scheduler.reload_capacity_config().await?;
    let probe = rd_files::PersistenceProbe::detect();
    Ok((StatusCode::CREATED, Json(with_persistence(&probe, value))))
}

#[utoipa::path(put, path = "/api/v1/storage-roots/{id}", tag = "configuration", params(("id" = rd_core::StorageRootId, Path)), request_body = CreateStorageRootRequest, responses((status = 200, body = crate::dto::StorageRootResponse), (status = 404)))]
pub async fn update_storage_root(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::StorageRootId>,
    Json(request): Json<CreateStorageRootRequest>,
) -> Result<Json<crate::dto::StorageRootResponse>, ApiError> {
    let input = validated_storage_root(id, request).await?;
    let value = state
        .database
        .update_storage_root(id, input)
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
        sfv_verify: request.sfv_verify,
        safe_postproc: request.safe_postproc,
        delete_par2: request.delete_par2,
        upload_enabled: request.upload_enabled,
        upload_remote: crate::dto::normalize_upload_remote(request.upload_remote)?,
    })
}

#[utoipa::path(post, path = "/api/v1/categories", tag = "configuration", request_body = CreateCategoryRequest, responses((status = 201, body = rd_core::Category)))]
pub async fn create_category(
    State(state): State<AppState>,
    Json(request): Json<CreateCategoryRequest>,
) -> Result<(StatusCode, Json<rd_core::Category>), ApiError> {
    let input = validated_category(&state, request).await?;
    let value = state.database.create_category(input).await?;
    Ok((StatusCode::CREATED, Json(value)))
}

#[utoipa::path(put, path = "/api/v1/categories/{id}", tag = "configuration", params(("id" = rd_core::CategoryId, Path)), request_body = CreateCategoryRequest, responses((status = 200, body = rd_core::Category), (status = 404)))]
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
                StoreErrorKind::InUse,
                "category.in_use",
                "The category is still used by unfinished packages",
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

#[utoipa::path(get, path = "/api/v1/hotfolders", tag = "configuration", responses((status = 200, body = [rd_core::HotFolderConfig])))]
pub async fn list_hotfolders(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::HotFolderConfig>>, ApiError> {
    Ok(Json(state.database.list_hotfolders().await?))
}

async fn validated_hotfolder(
    state: &AppState,
    request: CreateHotFolderRequest,
) -> Result<rd_db::NewHotFolder, ApiError> {
    validate_name(&request.name)?;
    if request.processed_path.trim().is_empty() || request.failed_path.trim().is_empty() {
        return Err(ApiError::bad_request(
            "hotfolder.subfolders_required",
            "Processed and failed paths must not be empty",
        ));
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
    if request.path.trim().is_empty() {
        return Err(ApiError::bad_request(
            "hotfolder.path_required",
            "Hotfolder path is required",
        ));
    }
    if matches!(request.executor, rd_core::HotFolderExecutor::Daemon) {
        let path = PathBuf::from(&request.path);
        if !path.is_absolute() {
            return Err(ApiError::bad_request(
                "hotfolder.path_not_absolute",
                "Daemon hotfolder path must be absolute",
            ));
        }
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

/// Rejects an account provider not in the registry, one that omits a required username, and
/// one whose credential mode does not match what the provider offers.
fn validate_provider(
    provider: &str,
    username: Option<&str>,
    mode: Option<rd_provider_registry::CredentialMode>,
) -> Result<(), ApiError> {
    let spec = rd_provider_registry::by_slug(provider).ok_or_else(|| {
        ApiError::bad_request(
            "account.provider_unsupported",
            "This provider is not supported",
        )
    })?;
    let offered = spec.credential_modes();
    match mode {
        Some(mode) if !offered.contains(&mode) => {
            return Err(ApiError::bad_request(
                "account.credential_mode_unsupported",
                "This provider does not offer that credential mode",
            ));
        }
        // Leaving it out would silently fall back to the provider's first mode, which decides
        // whether the stored secret is treated as a password or as an API key. Too consequential
        // to guess: the form asks, so the request carries it.
        None if !offered.is_empty() => {
            return Err(ApiError::bad_request(
                "account.credential_mode_required",
                "This provider requires a credential mode",
            ));
        }
        _ => {}
    }
    let has_username = username
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let username_required =
        spec.username_required || mode == Some(rd_provider_registry::CredentialMode::Login);
    if username_required && !has_username {
        return Err(ApiError::bad_request(
            "account.username_required",
            "This provider requires a username",
        ));
    }
    Ok(())
}

pub(crate) fn validate_name(value: &str) -> Result<(), ApiError> {
    let length = value.trim().chars().count();
    if !(1..=100).contains(&length) {
        return Err(ApiError::bad_request(
            "request.name_length",
            "Name must be between 1 and 100 characters long",
        )
        .with_param("min", 1)
        .with_param("max", 100));
    }
    Ok(())
}

fn validate_relative(value: &str, code: &'static str, label: &str) -> Result<(), ApiError> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(ApiError::bad_request(
            code,
            format!("{label} must be relative and must not contain path traversal"),
        )
        .with_param("label", label));
    }
    Ok(())
}

fn valid_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

pub(crate) fn validate_secret_value(
    value: Option<&str>,
    maximum: usize,
    code: &'static str,
    label: &str,
) -> Result<(), ApiError> {
    if let Some(value) = value
        && (value.is_empty() || value.len() > maximum)
    {
        return Err(ApiError::bad_request(
            code,
            format!("{label} is empty or exceeds the size limit of {maximum} bytes"),
        )
        .with_param("label", label)
        .with_param("max", maximum));
    }
    Ok(())
}

pub(crate) async fn store_optional(
    store: &rd_secrets::SecretStore,
    value: Option<String>,
) -> Result<Option<String>, ApiError> {
    match value.filter(|value| !value.is_empty()) {
        Some(value) => Ok(Some(store.put_string(value).await?)),
        None => Ok(None),
    }
}

pub(crate) async fn cleanup_secrets(
    store: &rd_secrets::SecretStore,
    references: impl IntoIterator<Item = Option<String>>,
) {
    for reference in references.into_iter().flatten() {
        if let Err(error) = store.remove(&reference).await {
            tracing::warn!(%error, "failed to clean up orphaned secret");
        }
    }
}

async fn cleanup_replaced_secret(
    store: &rd_secrets::SecretStore,
    old: Option<String>,
    new: &Option<String>,
) {
    if old
        .as_ref()
        .is_some_and(|reference| Some(reference) != new.as_ref())
    {
        cleanup_secrets(store, [old]).await;
    }
}

async fn validate_proxy_selection(
    state: &AppState,
    proxy_id: Option<rd_core::ProxyProfileId>,
) -> Result<(), ApiError> {
    if let Some(proxy_id) = proxy_id
        && !state
            .database
            .list_proxy_profiles()
            .await?
            .iter()
            .any(|profile| profile.id == proxy_id)
    {
        return Err(ApiError::bad_request(
            "proxy.not_found",
            "Proxy profile not found",
        ));
    }
    Ok(())
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn download_destination(
    state: &AppState,
    selected: Option<rd_core::CategoryId>,
) -> Result<Option<PathBuf>, ApiError> {
    crate::destination::resolve_destination(&state.database, selected)
        .await
        .map_err(|error| {
            ApiError::bad_request(
                "category.destination_unresolved",
                format!("Download destination could not be resolved: {error}"),
            )
            .with_param("reason", error.to_string())
        })
}

#[cfg(test)]
#[path = "config_handlers_check_tests.rs"]
mod check_tests;

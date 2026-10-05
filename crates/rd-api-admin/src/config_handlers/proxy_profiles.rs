//! Proxy profiles: list, create, change and delete.

use super::*;

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
pub(super) fn validated_proxy_profile(
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

pub(super) fn proxy_credentials_incomplete() -> ApiError {
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
    // points at. The global selection and the torrent engine's peer proxy live in the settings
    // document instead, so they are checked here rather than there; a deleted peer proxy would
    // leave the torrent session unable to start (RD-190-22).
    let settings = crate::settings_store::read_settings(&state).await?;
    if settings.global_proxy_profile_id == Some(id) || settings.torrent_proxy_profile_id == Some(id)
    {
        return Err(ApiError::conflict(
            "proxy.in_use",
            "The proxy profile is still selected as the global or the torrent proxy",
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

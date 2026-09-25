use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_db::StoreErrorKind;

use crate::{
    ApiError, AppState,
    dto::{
        CreateUsenetServerRequest, MessageResponse, NzbImportEnqueueRequest,
        UpdateUsenetServerRequest,
    },
};

#[utoipa::path(get, path = "/api/v1/usenet/servers", tag = "usenet", responses((status = 200, body = [rd_core::UsenetServer])))]
pub async fn list_usenet_servers(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::UsenetServer>>, ApiError> {
    Ok(Json(state.database.list_usenet_servers().await?))
}

#[utoipa::path(put, path = "/api/v1/usenet/servers/{id}", tag = "usenet", params(("id" = rd_core::UsenetServerId, Path)), request_body = UpdateUsenetServerRequest, responses((status = 200, body = rd_core::UsenetServer), (status = 404)))]
pub async fn update_usenet_server(
    State(state): State<AppState>,
    Path(id): Path<rd_core::UsenetServerId>,
    Json(request): Json<UpdateUsenetServerRequest>,
) -> Result<Json<rd_core::UsenetServer>, ApiError> {
    validate_server_fields(
        &request.name,
        &request.host,
        request.max_connections,
        request.password.as_deref(),
    )?;
    validate_socks_proxy(&state, request.proxy_profile_id).await?;
    let stored = state
        .database
        .usenet_connection_config(id)
        .await?
        .ok_or_else(crate::error_codes::usenet_server_not_found)?;
    let old_password_ref = stored.password_ref;
    let password_ref = match request.password {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if request.clear_password => None,
        None => old_password_ref.clone(),
    };
    let username = normalized(request.username);
    if username.is_some() != password_ref.is_some() {
        if password_ref != old_password_ref {
            cleanup_secret(&state.secrets, password_ref).await;
        }
        return Err(ApiError::bad_request(
            "usenet.credentials_incomplete",
            "NNTP username and password must be set together",
        ));
    }
    let result = state
        .database
        .update_usenet_server(
            id,
            rd_db::UpdateUsenetServer {
                name: request.name.trim().to_owned(),
                host: request.host.trim().to_ascii_lowercase(),
                port: request.port,
                tls: request.tls,
                username,
                password_ref: password_ref.clone(),
                proxy_profile_id: request.proxy_profile_id,
                priority: request.priority,
                max_connections: request.max_connections,
                enabled: request.enabled,
            },
        )
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            if password_ref != old_password_ref {
                cleanup_secret(&state.secrets, password_ref).await;
            }
            return Err(error.into());
        }
    };
    if old_password_ref != password_ref {
        cleanup_secret(&state.secrets, old_password_ref).await;
    }
    Ok(Json(value))
}

#[utoipa::path(delete, path = "/api/v1/usenet/servers/{id}", tag = "usenet", params(("id" = rd_core::UsenetServerId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn delete_usenet_server(
    State(state): State<AppState>,
    Path(id): Path<rd_core::UsenetServerId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let password_ref = state
        .database
        .delete_usenet_server(id)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => crate::error_codes::usenet_server_not_found(),
            _ => error.into(),
        })?;
    cleanup_secret(&state.secrets, password_ref).await;
    Ok(Json(MessageResponse::new(
        "usenet.server_deleted",
        "NNTP server deleted",
    )))
}

#[utoipa::path(get, path = "/api/v1/nzb/imports/{id}/files", tag = "usenet", params(("id" = rd_core::NzbImportId, Path)), responses((status = 200, body = [rd_core::NzbFileStatus])))]
pub async fn list_nzb_files(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
) -> Result<Json<Vec<rd_core::NzbFileStatus>>, ApiError> {
    Ok(Json(state.database.list_nzb_files(id).await?))
}

#[utoipa::path(get, path = "/api/v1/nzb/imports/{id}/postprocess", tag = "usenet", params(("id" = rd_core::NzbImportId, Path)), responses((status = 200, body = [rd_core::PostprocessStep])))]
pub async fn list_postprocess_steps(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
) -> Result<Json<Vec<rd_core::PostprocessStep>>, ApiError> {
    Ok(Json(
        state
            .database
            .list_postprocess_steps(&id.to_string())
            .await?,
    ))
}

/// Moves one imported NZB into the download queue.
///
/// The body is optional so a caller that never sent one keeps working; `paused` creates the
/// package with every download paused, which is what "add paused" in the LinkGrabber promises
/// for NZBs just as it does for links (RD-107-09).
#[utoipa::path(post, path = "/api/v1/nzb/imports/{id}/enqueue", tag = "usenet", params(("id" = rd_core::NzbImportId, Path)), request_body = Option<NzbImportEnqueueRequest>, responses((status = 201, body = rd_core::DownloadPackage), (status = 404), (status = 409)))]
pub async fn enqueue_nzb_import(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
    request: Option<Json<NzbImportEnqueueRequest>>,
) -> Result<(axum::http::StatusCode, Json<rd_core::DownloadPackage>), ApiError> {
    let request = request.map(|Json(body)| body).unwrap_or_default();
    let import = state
        .database
        .list_nzb_imports()
        .await?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ApiError::not_found("nzb.import_not_found", "NZB import not found"))?;
    if import.state == rd_core::NzbImportState::Enqueued {
        return Err(nzb_already_enqueued());
    }
    // A failed import has no files to queue; it is the record of a drop that did not work
    // (RD-108-20). Deleting it is what makes room for the file to be imported again.
    if import.state == rd_core::NzbImportState::Failed {
        return Err(ApiError::conflict(
            "nzb.import_failed",
            "NZB import failed and cannot be queued",
        ));
    }
    let destination = crate::config_handlers::download_destination(&state, import.category_id)
        .await?
        .unwrap_or_else(|| state.scheduler.downloads_directory().to_path_buf());
    let package = state
        .database
        .enqueue_nzb_import(
            id,
            destination,
            rd_core::DownloadPriority::Normal,
            request.paused,
        )
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::WrongState) => nzb_already_enqueued(),
            _ => error.into(),
        })?;
    Ok((axum::http::StatusCode::CREATED, Json(package)))
}

fn nzb_already_enqueued() -> ApiError {
    ApiError::conflict(
        "nzb.already_enqueued",
        "NZB import is already in the download list",
    )
}

#[utoipa::path(post, path = "/api/v1/usenet/servers", tag = "usenet", request_body = CreateUsenetServerRequest, responses((status = 201, body = rd_core::UsenetServer)))]
pub async fn create_usenet_server(
    State(state): State<AppState>,
    Json(request): Json<CreateUsenetServerRequest>,
) -> Result<(StatusCode, Json<rd_core::UsenetServer>), ApiError> {
    validate_server_fields(
        &request.name,
        &request.host,
        request.max_connections,
        request.password.as_deref(),
    )?;
    let host = request.host.trim();
    let username = normalized(request.username);
    if username.is_some() != request.password.is_some() {
        return Err(ApiError::bad_request(
            "usenet.credentials_incomplete",
            "NNTP username and password must be set together",
        ));
    }
    validate_socks_proxy(&state, request.proxy_profile_id).await?;
    let password_ref = match request.password {
        Some(password) => Some(state.secrets.put_string(password).await?),
        None => None,
    };
    let result = state
        .database
        .create_usenet_server(rd_db::NewUsenetServer {
            name: request.name.trim().to_owned(),
            host: host.to_ascii_lowercase(),
            port: request.port,
            tls: request.tls,
            username,
            password_ref: password_ref.clone(),
            proxy_profile_id: request.proxy_profile_id,
            priority: request.priority,
            max_connections: request.max_connections,
            enabled: request.enabled,
        })
        .await;
    match result {
        Ok(value) => Ok((StatusCode::CREATED, Json(value))),
        Err(error) => {
            if let Some(reference) = password_ref
                && let Err(cleanup_error) = state.secrets.remove(&reference).await
            {
                tracing::warn!(%cleanup_error, "failed to clean up orphaned NNTP secret");
            }
            Err(error.into())
        }
    }
}

#[utoipa::path(post, path = "/api/v1/usenet/servers/{id}/test", tag = "usenet", params(("id" = rd_core::UsenetServerId, Path)), responses((status = 200, body = MessageResponse)))]
pub async fn test_usenet_server(
    State(state): State<AppState>,
    Path(id): Path<rd_core::UsenetServerId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let stored = state
        .database
        .usenet_connection_config(id)
        .await?
        .ok_or_else(crate::error_codes::usenet_server_not_found)?;
    if !stored.server.enabled {
        return Err(ApiError::bad_request(
            "usenet.server_disabled",
            "NNTP server is disabled",
        ));
    }
    // The same trust roots the runner will use. Testing with the platform store alone made
    // this answer "cannot connect" for a server the downloads then reached without trouble.
    let custom_ca_pem = state
        .scheduler
        .network_defaults()
        .read()
        .await
        .custom_ca_pem
        .clone();
    let config =
        rd_usenet::server_config_with_ca(&state.database, &state.secrets, id, &custom_ca_pem)
            .await?
            .ok_or_else(crate::error_codes::usenet_server_not_found)?;
    tokio::time::timeout(
        std::time::Duration::from_secs(35),
        rd_usenet::NntpClient::connect(&config),
    )
    .await
    .map_err(|_| ApiError::bad_gateway("usenet.connection_timeout", "NNTP connection timed out"))?
    .map_err(|error| nntp_test_error(&error))?;
    let name = stored.server.name;
    Ok(Json(
        MessageResponse::new(
            "usenet.connection_ok",
            format!("NNTP connection to {name} succeeded"),
        )
        .with_param("server", &name),
    ))
}

/// Turns a failed connection attempt into the refusal the interface translates.
///
/// The status code is read as a number now, not found as a substring of our own sentence. The
/// phrase after it stays a text match on purpose: it is the *provider's* wording, not ours, and
/// there is nothing to type on this side of it.
fn nntp_test_error(error: &anyhow::Error) -> ApiError {
    let message = error.to_string();
    let denied = rd_usenet::nntp_status(error).is_some_and(|status| {
        status.code() == 502
            && status
                .text()
                .to_ascii_lowercase()
                .contains("access denied to your node")
    });
    if denied {
        return ApiError::bad_gateway(
            "usenet.access_denied",
            "Premiumize rejects the public IP of this system (NNTP 502: Access denied to your node). This is not a local connection error. Check that the server uses the intended internet connection, or contact support@premiumize.me with the public IP",
        );
    }
    ApiError::bad_gateway(
        "usenet.connection_failed",
        format!("NNTP connection failed: {message}"),
    )
    .with_param("detail", message)
}

async fn validate_socks_proxy(
    state: &AppState,
    proxy_id: Option<rd_core::ProxyProfileId>,
) -> Result<(), ApiError> {
    let Some(proxy_id) = proxy_id else {
        return Ok(());
    };
    let proxy = state
        .database
        .list_proxy_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.id == proxy_id)
        .ok_or_else(|| ApiError::bad_request("proxy.not_found", "Proxy profile not found"))?;
    if !matches!(proxy.kind, rd_core::ProxyKind::Socks5) {
        return Err(ApiError::bad_request(
            "usenet.proxy_not_socks5",
            "NNTP only supports SOCKS5 proxies",
        ));
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<(), ApiError> {
    if !(1..=100).contains(&value.trim().chars().count()) {
        return Err(ApiError::bad_request(
            "usenet.name_length",
            "Name must be between 1 and 100 characters",
        )
        .with_param("max", 100));
    }
    Ok(())
}

fn validate_secret(value: Option<&str>) -> Result<(), ApiError> {
    if value.is_some_and(|value| value.is_empty() || value.len() > 16 * 1024) {
        return Err(ApiError::bad_request(
            "usenet.password_invalid",
            "NNTP password is empty or exceeds the size limit",
        ));
    }
    Ok(())
}

fn validate_server_fields(
    name: &str,
    host: &str,
    max_connections: u16,
    password: Option<&str>,
) -> Result<(), ApiError> {
    validate_name(name)?;
    let host = host.trim();
    if host.is_empty() || url::Host::parse(host).is_err() {
        return Err(ApiError::bad_request(
            "usenet.host_invalid",
            "NNTP hostname is invalid",
        ));
    }
    if !(1..=32).contains(&max_connections) {
        return Err(ApiError::bad_request(
            "usenet.connections_range",
            "NNTP connections must be between 1 and 32",
        )
        .with_param("min", 1)
        .with_param("max", 32));
    }
    validate_secret(password)
}

async fn cleanup_secret(store: &rd_secrets::SecretStore, reference: Option<String>) {
    if let Some(reference) = reference
        && let Err(error) = store.remove(&reference).await
    {
        tracing::warn!(%error, "failed to clean up replaced NNTP secret");
    }
}

fn normalized(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

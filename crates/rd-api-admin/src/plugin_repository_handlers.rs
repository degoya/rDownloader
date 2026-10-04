//! Plugin repositories, updates and the install preview (RD-140-01).
//!
//! A repository delivers packages; it never makes one trustworthy. Adding a third-party
//! repository approves *its* key the way an unknown plugin key is approved — refused once with
//! the fingerprint, confirmed by sending that fingerprint back — and every package installed
//! from any repository still goes through `PluginInstaller::install_bytes` under a trusted
//! plugin key, after its bytes have matched the index's digest.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_plugin_host::{
    index::Permissions,
    repository::{Offer, REFRESH_HOURS_RANGE, RepositoryError, is_official},
};

use crate::{
    ApiError, AppState,
    audit::{Actor, AuditContext, AuditEvent},
    dto::MessageResponse,
    plugin_handlers::{
        MAX_PLUGIN_PACKAGE_BYTES, confirm_signing_key, install_error, installed_message,
        register_installed,
    },
    plugin_repository_dto::{
        AddPluginRepositoryQuery, AddPluginRepositoryRequest, PluginOffersResponse,
        PluginPreviewResponse, PluginRepositoriesResponse, PluginRepositoryResponse,
        PluginRepositorySettingsRequest, RepositoryPackageRequest, UpdatePluginRepositoryRequest,
    },
};

/// Longest repository name.
const MAX_NAME_CHARS: usize = 120;

#[utoipa::path(get, path = "/api/v1/plugins/repositories", tag = "plugins", responses((status = 200, body = PluginRepositoriesResponse)))]
pub async fn list_plugin_repositories(
    State(state): State<AppState>,
) -> Result<Json<PluginRepositoriesResponse>, ApiError> {
    Ok(Json(repositories(&state).await?))
}

/// Adds a third-party repository once its key is approved.
///
/// The first request fetches the index and checks that the pasted key signs it, then refuses
/// with `409 plugin_repository.key_unconfirmed` naming the key id and fingerprint. The client
/// shows both and sends the same request again with `trust_fingerprint`; only an exact match
/// adds the repository, so the key approved is the key the person saw.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/repositories",
    tag = "plugins",
    params(AddPluginRepositoryQuery),
    request_body = AddPluginRepositoryRequest,
    responses(
        (status = 201, body = PluginRepositoryResponse),
        (status = 409, description = "The repository key has not been confirmed yet", body = MessageResponse)
    )
)]
pub async fn add_plugin_repository(
    State(state): State<AppState>,
    audit: AuditContext,
    Query(query): Query<AddPluginRepositoryQuery>,
    Json(request): Json<AddPluginRepositoryRequest>,
) -> Result<(StatusCode, Json<PluginRepositoryResponse>), ApiError> {
    let probe = state
        .plugin_repositories
        .probe(&request.url, &request.public_key)
        .await
        .map_err(repository_error)?;
    match query.trust_fingerprint.as_deref().map(str::trim) {
        Some(confirmed) if confirmed.eq_ignore_ascii_case(&probe.fingerprint) => {}
        Some(_) => {
            return Err(ApiError::bad_request(
                "plugin_repository.key_fingerprint_mismatch",
                "The confirmed fingerprint does not match the repository key",
            ));
        }
        None => {
            return Err(ApiError::conflict(
                "plugin_repository.key_unconfirmed",
                format!(
                    "Confirm the repository key {} before adding the repository",
                    probe.key_id
                ),
            )
            .with_param("key_id", probe.key_id.clone())
            .with_param("fingerprint", probe.fingerprint.clone())
            .with_param("packages", probe.package_count())
            .with_param("url", probe.url.to_string()));
        }
    }
    let name = match request.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => valid_name(name)?,
        _ => probe.url.host_str().unwrap_or_default().to_owned(),
    };
    let fingerprint = probe.fingerprint.clone();
    let key_id = probe.key_id.clone();
    let repository = state
        .plugin_repositories
        .add(name, probe)
        .await
        .map_err(repository_error)?;
    // A trust decision like confirming a plugin key: from here on, this repository's index is
    // believed about what it offers and what it withdraws from what it delivered.
    crate::audit::record(
        &state,
        AuditEvent::success(rd_core::AuditAction::PluginRepositoryAdded)
            .by(&audit)
            .target("plugin_repository", &repository.id)
            .named(repository.name.clone())
            .detail("key_id", key_id)
            .detail("confirmed_key", fingerprint),
    )
    .await;
    let expires = state.plugin_repositories.index_expiry(&repository.id);
    Ok((
        StatusCode::CREATED,
        Json(PluginRepositoryResponse::from_row(repository, expires)),
    ))
}

/// Switches a repository on or off, or renames it.
///
/// Off stops refreshes, offers and updates from it. Nothing installed from it is touched.
#[utoipa::path(
    patch,
    path = "/api/v1/plugins/repositories/{id}",
    tag = "plugins",
    params(("id" = String, Path, description = "Repository id")),
    request_body = UpdatePluginRepositoryRequest,
    responses((status = 200, body = MessageResponse), (status = 404, body = MessageResponse))
)]
pub async fn update_plugin_repository(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<UpdatePluginRepositoryRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let name = match request.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => Some(valid_name(name)?),
        _ => None,
    };
    let found = state
        .database
        .update_plugin_repository(id.clone(), request.enabled, name)
        .await?;
    if !found {
        return Err(not_found());
    }
    let mut event = AuditEvent::success(rd_core::AuditAction::PluginRepositoryChanged)
        .by(&audit)
        .target("plugin_repository", &id);
    if let Some(enabled) = request.enabled {
        event = event.detail("enabled", enabled);
    }
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new("plugin_repository.updated", "Repository updated")
            .with_param("id", id),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/plugins/repositories/{id}",
    tag = "plugins",
    params(("id" = String, Path, description = "Repository id")),
    responses((status = 200, body = MessageResponse), (status = 400, body = MessageResponse), (status = 404, body = MessageResponse))
)]
pub async fn remove_plugin_repository(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    if is_official(&id) {
        return Err(ApiError::bad_request(
            "plugin_repository.official_permanent",
            "The official repository cannot be removed; switch it off instead",
        ));
    }
    if !state
        .plugin_repositories
        .remove(&id)
        .await
        .map_err(repository_error)?
    {
        return Err(not_found());
    }
    crate::audit::record(
        &state,
        AuditEvent::success(rd_core::AuditAction::PluginRepositoryRemoved)
            .by(&audit)
            .target("plugin_repository", &id),
    )
    .await;
    Ok(Json(
        MessageResponse::new(
            "plugin_repository.removed",
            "Repository removed; plugins installed from it stay installed",
        )
        .with_param("id", id),
    ))
}

/// Refreshes every enabled repository now, then installs the updates set to automatic.
#[utoipa::path(post, path = "/api/v1/plugins/repositories/refresh", tag = "plugins", responses((status = 200, body = PluginRepositoriesResponse)))]
pub async fn refresh_plugin_repositories(
    State(state): State<AppState>,
    audit: AuditContext,
) -> Result<Json<PluginRepositoriesResponse>, ApiError> {
    refresh_and_update(&state, audit.actor).await;
    Ok(Json(repositories(&state).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/plugins/repositories/settings",
    tag = "plugins",
    request_body = PluginRepositorySettingsRequest,
    responses((status = 200, body = PluginRepositoriesResponse), (status = 400, body = MessageResponse))
)]
pub async fn set_plugin_repository_settings(
    State(state): State<AppState>,
    Json(request): Json<PluginRepositorySettingsRequest>,
) -> Result<Json<PluginRepositoriesResponse>, ApiError> {
    if !REFRESH_HOURS_RANGE.contains(&request.refresh_hours) {
        return Err(ApiError::bad_request(
            "plugin_repository.refresh_hours_invalid",
            format!(
                "The refresh interval is between {} and {} hours",
                REFRESH_HOURS_RANGE.start(),
                REFRESH_HOURS_RANGE.end()
            ),
        )
        .with_param("min", REFRESH_HOURS_RANGE.start())
        .with_param("max", REFRESH_HOURS_RANGE.end()));
    }
    state
        .plugin_repositories
        .set_refresh_hours(request.refresh_hours)
        .await?;
    Ok(Json(repositories(&state).await?))
}

/// Updates for installed plugins and what else the enabled repositories offer.
#[utoipa::path(get, path = "/api/v1/plugins/updates", tag = "plugins", responses((status = 200, body = PluginOffersResponse)))]
pub async fn list_plugin_updates(
    State(state): State<AppState>,
) -> Result<Json<PluginOffersResponse>, ApiError> {
    let service = &state.plugin_repositories;
    let updates = service.updates().await?;
    let (installed, available): (Vec<_>, Vec<_>) = service
        .offers()
        .await?
        .into_iter()
        .partition(|offer| offer.installed_version.is_some());
    Ok(Json(PluginOffersResponse {
        updates: updates.into_iter().map(Into::into).collect(),
        available: available.into_iter().map(Into::into).collect(),
        installed: installed.into_iter().map(Into::into).collect(),
    }))
}

/// Describes an uploaded `.rdplug` without installing it: name, version, publisher,
/// permissions and whether its key is trusted.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/preview",
    tag = "plugins",
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 200, body = PluginPreviewResponse), (status = 400, body = MessageResponse))
)]
pub async fn preview_plugin_package(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    bytes: Bytes,
) -> Result<Json<PluginPreviewResponse>, ApiError> {
    rd_api_core::input_checks::require_media_type(&headers, "application/octet-stream")?;
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_PACKAGE_BYTES {
        return Err(ApiError::bad_request(
            "plugin.package_size_invalid",
            format!(
                "The .rdplug package must be between 1 byte and {MAX_PLUGIN_PACKAGE_BYTES} bytes"
            ),
        )
        .with_param("max", MAX_PLUGIN_PACKAGE_BYTES));
    }
    Ok(Json(preview(&state, bytes.to_vec()).await?))
}

/// Downloads one offered package, proves it matches the index, and describes it like an upload.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/repositories/{id}/preview",
    tag = "plugins",
    params(("id" = String, Path, description = "Repository id")),
    request_body = RepositoryPackageRequest,
    responses((status = 200, body = PluginPreviewResponse), (status = 404, body = MessageResponse), (status = 502, body = MessageResponse))
)]
pub async fn preview_repository_package(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RepositoryPackageRequest>,
) -> Result<Json<PluginPreviewResponse>, ApiError> {
    let (offer, bytes) = state
        .plugin_repositories
        .download(&id, &request.plugin_id, &request.version)
        .await
        .map_err(repository_error)?;
    Ok(Json(preview(&state, bytes).await?.with_offer(&offer)))
}

/// Installs one offered package. A first install runs at once; an update from the next start
/// (RD-170-12).
#[utoipa::path(
    post,
    path = "/api/v1/plugins/repositories/{id}/install",
    tag = "plugins",
    params(("id" = String, Path, description = "Repository id")),
    request_body = RepositoryPackageRequest,
    responses(
        (status = 201, body = MessageResponse),
        (status = 409, description = "Signed by a key the user has not confirmed yet", body = MessageResponse),
        (status = 502, body = MessageResponse)
    )
)]
pub async fn install_repository_package(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<RepositoryPackageRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let (offer, bytes) = state
        .plugin_repositories
        .download(&id, &request.plugin_id, &request.version)
        .await
        .map_err(repository_error)?;
    if let Some(confirmed) = request.trust_fingerprint.as_deref() {
        confirm_signing_key(&state, &Bytes::from(bytes.clone()), confirmed).await?;
    }
    let message = install_offer(
        &state,
        &offer,
        bytes,
        audit.actor,
        request.trust_fingerprint,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(message)))
}

/// Loads the cached plugin indexes that still verify and starts the refresh loop
/// (RD-140-01).
///
/// Separate from the constructor for the reason [`AppState::prepare_managed_tools`] is: it reads
/// files and starts a task. The first refresh waits
/// [`STARTUP_DELAY`](rd_plugin_host::repository::STARTUP_DELAY), so fetching an index never
/// delays the start, and a failed refresh never stops anything else.
pub async fn prepare_plugin_repositories(state: &AppState) {
    state.plugin_repositories.load().await;
    let state = state.clone();
    tokio::spawn(async move {
        use rd_plugin_host::repository::STARTUP_DELAY;
        // Checked every ten minutes rather than slept for the whole interval, so a shorter
        // interval set in the meantime applies without a restart.
        const TICK: std::time::Duration = std::time::Duration::from_secs(600);
        tokio::time::sleep(STARTUP_DELAY).await;
        let mut last = None::<std::time::Instant>;
        loop {
            let hours = u64::from(state.plugin_repositories.refresh_hours().await);
            let due = last
                .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(hours * 3600));
            if due {
                refresh_and_update(&state, Actor::system()).await;
                last = Some(std::time::Instant::now());
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Refreshes every enabled repository, then installs each update whose plugin is set to
/// automatic. Called by the refresh route and by the background loop; never fails, because
/// every outcome is recorded on its repository's row.
pub(crate) async fn refresh_and_update(state: &AppState, actor: Actor) {
    state.plugin_repositories.refresh_all().await;
    let updates = match state.plugin_repositories.updates().await {
        Ok(updates) => updates,
        Err(error) => {
            tracing::warn!(%error, "could not compute plugin updates");
            return;
        }
    };
    // Automatic and asking for nothing new: an update that widens the permissions is listed
    // with them and installed on a click, never granted unseen. The ones that wait for that
    // click are announced (RD-190-19), once per plugin and version however often this runs.
    let (updates, waiting): (Vec<_>, Vec<_>) = updates
        .into_iter()
        .partition(rd_plugin_host::repository::Update::installs_itself);
    for update in waiting {
        rd_api_core::notify_notice::announce(
            &state.database,
            rd_api_core::notify_notice::Notice::plugin_update_available(
                &update.offer.entry.id.to_string(),
                &update.offer.entry.name,
                &update.installed_version,
                &update.offer.entry.version,
            ),
        )
        .await;
    }
    for update in updates {
        let offer = update.offer;
        let outcome = match state
            .plugin_repositories
            .download(
                &offer.repository_id,
                &offer.entry.id.to_string(),
                &offer.entry.version,
            )
            .await
        {
            // No key confirmation here: an automatic update installs only under a key that is
            // already trusted, and one that is not waits for somebody to confirm it by hand.
            // `download` has already held the entry's publisher to the package's signature, so
            // the key is the one the installed version is signed with, not the index's word.
            Ok((offer, bytes)) => install_offer(state, &offer, bytes, actor.clone(), None)
                .await
                .map(|_| ()),
            Err(error) => Err(repository_error(error)),
        };
        if let Err(error) = outcome {
            tracing::warn!(
                plugin = %offer.entry.name,
                version = %offer.entry.version,
                code = error.code(),
                "automatic plugin update was not installed"
            );
            // The next refresh tries the same version again; the notice goes out once.
            rd_api_core::notify_notice::announce(
                &state.database,
                rd_api_core::notify_notice::Notice::plugin_update_failed(
                    &offer.entry.id.to_string(),
                    &offer.entry.name,
                    &offer.entry.version,
                    error.code(),
                ),
            )
            .await;
        }
    }
}

/// Installs downloaded, digest-checked bytes and records where they came from.
async fn install_offer(
    state: &AppState,
    offer: &Offer,
    bytes: Vec<u8>,
    actor: Actor,
    confirmed_key: Option<String>,
) -> Result<MessageResponse, ApiError> {
    let installed = state
        .plugins
        .install_bytes(bytes)
        .await
        .map_err(install_error)?;
    let running = register_installed(state, &installed).await?;
    // The version folder exists from here on. A stop at either point below leaves it installed
    // and the pointers where they were (RD-180-12, recovery matrix).
    rd_core::failpoint!("plugin.before_install_recorded", || ApiError::from(
        anyhow::anyhow!("crash point")
    ));
    if let Err(error) = state.plugin_repositories.record_install(offer).await {
        // The install stands; only a later withdrawal by a third-party repository loses its
        // reach over this version, which is the narrower of the two failures.
        tracing::warn!(%error, "could not record which repository a plugin came from");
    }
    // Like the source record: the install stands even when the pointers could not follow it,
    // and the plugin manager still offers the new version to activate by hand.
    let id = installed.manifest.id.to_string();
    let version = &installed.manifest.version;
    rd_core::failpoint!("plugin.before_pointers_followed", || ApiError::from(
        anyhow::anyhow!("crash point")
    ));
    if let Err(error) = crate::plugin_update_policy::follow_update(state, &id, version).await {
        tracing::warn!(
            code = error.code(),
            "the version pointers did not follow a plugin update"
        );
    }
    let mut event = AuditEvent::success(rd_core::AuditAction::PluginInstalled)
        .actor(actor)
        .target("plugin", installed.manifest.id)
        .named(installed.manifest.name.clone())
        .detail("version", &installed.manifest.version)
        .detail("repository", &offer.repository_id);
    if let Some(fingerprint) = confirmed_key {
        event = event.detail("confirmed_key", fingerprint);
    }
    crate::audit::record(state, event).await;
    Ok(installed_message(&installed, running))
}

async fn preview(state: &AppState, bytes: Vec<u8>) -> Result<PluginPreviewResponse, ApiError> {
    let verifier = state.plugins.verifier().clone();
    let preview = tokio::task::spawn_blocking(move || {
        rd_plugin_host::preview::preview_package(&bytes, &verifier)
    })
    .await
    .map_err(anyhow::Error::new)?
    .map_err(|error| {
        ApiError::bad_request(
            "plugin.preview_failed",
            format!("The package cannot be read: {error:#}"),
        )
        .with_param("reason", format!("{error:#}"))
    })?;
    let id = preview.id.to_string();
    let installed = state
        .plugins
        .list_installed()
        .await?
        .into_iter()
        .filter(|manifest| manifest.id.to_string() == id)
        .map(|manifest| {
            let permissions = Permissions::of(&manifest);
            (manifest.version, permissions)
        })
        .collect();
    Ok(PluginPreviewResponse::new(preview, installed))
}

async fn repositories(state: &AppState) -> Result<PluginRepositoriesResponse, ApiError> {
    let service = &state.plugin_repositories;
    let repositories = state
        .database
        .list_plugin_repositories()
        .await?
        .into_iter()
        .map(|row| {
            let expires = service.index_expiry(&row.id);
            PluginRepositoryResponse::from_row(row, expires)
        })
        .collect();
    Ok(PluginRepositoriesResponse {
        repositories,
        refresh_hours: service.refresh_hours().await,
    })
}

fn valid_name(name: &str) -> Result<String, ApiError> {
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "plugin_repository.name_invalid",
            format!("A repository name is at most {MAX_NAME_CHARS} characters of plain text"),
        )
        .with_param("max", MAX_NAME_CHARS));
    }
    Ok(name.to_owned())
}

fn not_found() -> ApiError {
    ApiError::not_found(
        "plugin_repository.not_found",
        "The plugin repository does not exist",
    )
}

/// The status matters as much as the code: an index or a package that does not verify is an
/// upstream problem the client cannot fix by asking differently.
fn repository_error(error: RepositoryError) -> ApiError {
    let code = error.code();
    let message = error.to_string();
    match error {
        RepositoryError::NotFound | RepositoryError::NotOffered { .. } => {
            ApiError::not_found(code, message)
        }
        RepositoryError::Disabled | RepositoryError::AlreadyAdded => {
            ApiError::conflict(code, message)
        }
        RepositoryError::InvalidUrl
        | RepositoryError::InvalidKey
        | RepositoryError::KeyDoesNotSign => ApiError::bad_request(code, message),
        RepositoryError::Index(_)
        | RepositoryError::Download(_)
        | RepositoryError::DigestMismatch
        | RepositoryError::NotAsDescribed(_)
        | RepositoryError::OfficialKeyMissing => ApiError::bad_gateway(code, message),
        RepositoryError::Other(other) => other.into(),
    }
}

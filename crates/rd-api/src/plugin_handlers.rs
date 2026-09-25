//! Plugin manager endpoints: listing, trust-on-first-use installation and key management.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_plugin_host::VerifyError;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use crate::{
    ApiError, AppState,
    dto::{
        IncompatiblePluginResponse, InstalledPluginResponse, MessageResponse,
        PluginDigestRevocationResponse, PluginExecutionResponse, PluginInventoryResponse,
        PluginTrustedKeyResponse,
    },
};

const MAX_PLUGIN_PACKAGE_BYTES: usize = 65 * 1024 * 1024;

/// Confirmation of the fingerprint the client was shown for an unknown signing key.
#[derive(Deserialize, IntoParams)]
pub struct InstallPluginQuery {
    /// Hex SHA-256 of the package's public key, exactly as the 409 response reported it.
    #[serde(default)]
    trust_fingerprint: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/plugins", tag = "plugins", responses((status = 200, body = PluginInventoryResponse)))]
pub async fn list_plugins(
    State(state): State<AppState>,
) -> Result<Json<PluginInventoryResponse>, ApiError> {
    // `list_installed` is sorted by name and then by descending version, so the first entry for
    // an id is the one that wins at load time. Marking it here is what tells a leftover older
    // version apart from a plugin that is genuinely in use — they looked identical before.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // One grouped read for every plugin's number of recorded invocations, so the manager can
    // decide whether to offer the diagnostics accordion at all without fetching a single entry.
    // The entries themselves stay on demand; this is the count, not the history.
    let counts: std::collections::HashMap<rd_core::PluginId, i64> = state
        .database
        .plugin_execution_counts()
        .await?
        .into_iter()
        // The store keys entries by the id as text; a row whose id no longer parses belongs to
        // no installed plugin and is dropped rather than failing the inventory.
        .filter_map(|(id, count)| id.parse::<rd_core::PluginId>().ok().map(|id| (id, count)))
        .collect();
    let installed: Vec<InstalledPluginResponse> = state
        .plugins
        .list_installed()
        .await?
        .into_iter()
        .map(|manifest| {
            let mut response = InstalledPluginResponse::from(manifest);
            response.active = seen.insert(response.id.to_string());
            response.execution_count = counts
                .get(&response.id)
                .copied()
                .unwrap_or(0)
                .try_into()
                .unwrap_or(u32::MAX);
            response
        })
        .collect();
    let incompatible = state
        .plugins
        .list_incompatible()
        .await?
        .into_iter()
        .map(IncompatiblePluginResponse::from)
        .collect();
    Ok(Json(PluginInventoryResponse {
        installed,
        incompatible,
    }))
}

/// The newest recorded invocations of one plugin.
///
/// Bounded by the store itself, so this cannot ask for an unbounded read.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{id}/executions",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    responses((status = 200, body = [PluginExecutionResponse]))
)]
pub async fn list_plugin_executions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<PluginExecutionResponse>>, ApiError> {
    let entries = state
        .database
        .plugin_executions(&id, rd_db::MAX_EXECUTIONS_PER_PLUGIN)
        .await?
        .into_iter()
        .map(PluginExecutionResponse::from)
        .collect();
    Ok(Json(entries))
}

/// Removes one installed plugin version.
///
/// The manager offers this for a package this build refuses, so an outdated third-party
/// resolver can be cleared out by hand, and for the older of two installed versions, which
/// installing never removes. Nothing removes such a package on its own: it is the user's
/// artefact, and a plugin that vanishes without a word explains nothing.
///
/// Work that is still bound to the version refuses the removal instead. The whole reason an
/// older version survives an upgrade is that a job already under way keeps the version that
/// started it; taking it away by hand would break exactly what keeping it protects.
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{id}/{version}",
    tag = "plugins",
    params(
        ("id" = String, Path, description = "Plugin id"),
        ("version" = String, Path, description = "Installed version")
    ),
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = MessageResponse),
        (status = 409, body = MessageResponse, description = "Unfinished work is bound to this version")
    )
)]
pub async fn remove_plugin_version(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<MessageResponse>, ApiError> {
    // Asked before the directory goes: a pinned job and a transfer checkpoint both name one
    // exact version, and neither can be rebuilt from the newer one. The blockers are read
    // first, because naming one of them is what lets the reader go and look at it; the count
    // then says how many more there are.
    let blockers = state
        .database
        .plugin_version_blockers(&id, &version, 2)
        .await?;
    if !blockers.is_empty() {
        let names = blockers.join(", ");
        // The two reads are one predicate apart in time; a job that ended in between must not
        // turn the sentence into "0 unfinished downloads".
        let count = state
            .database
            .plugin_version_usage(&id, &version)
            .await?
            .max(1);
        let message = if count == 1 {
            format!("This plugin version cannot be removed: {names} is still using it")
        } else {
            format!(
                "This plugin version cannot be removed: {count} unfinished downloads are still using it, including {names}"
            )
        };
        return Err(ApiError::conflict("plugin.version_in_use", message)
            .with_param("count", count)
            .with_param("names", names));
    }
    let removed = state
        .plugins
        .remove_version(&id, &version)
        .await
        .map_err(|error| ApiError::bad_request("plugin.remove_failed", format!("{error:#}")))?;
    if !removed {
        return Err(ApiError::not_found(
            "plugin.not_installed",
            "This plugin version is not installed",
        ));
    }
    // The resolver itself goes on the next start, but the provider row must go now: it is what
    // the accounts list offers, and offering a provider whose plugin was just removed would
    // let an account be created that nothing can serve. Installing has always registered its
    // row immediately; removing simply never took it back.
    state.plugins.refresh_providers().await;
    announce_plugin(&state, &id, "removed");
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginRemoved)
            .by(&audit)
            .target("plugin", &id)
            .detail("version", &version),
    )
    .await;
    Ok(Json(MessageResponse::new(
        "plugin.removed",
        "Plugin version removed. Restart to apply.",
    )))
}

/// Tells open clients that the installed set changed.
///
/// `PluginChanged` had no producer at all until this: installing, removing and switching a
/// plugin off are filesystem and settings writes that never pass the database writer, so nothing
/// on the bus ever mentioned them. A plugin installed in one browser tab therefore stayed
/// invisible in every other until somebody reloaded, while the far narrower trust writes — which
/// do go through the writer — announced themselves. It carries the plugin id and what happened,
/// never a manifest: `PluginChanged` is administration-scoped and a manifest names domains and
/// secret slots.
fn announce_plugin(state: &AppState, id: &str, action: &str) {
    // Three events for one write, which is not redundancy. A subscriber receives an event only
    // when it holds that event's exact scope, and this write invalidates lists read at three
    // different ones: the plugin inventory at `Admin`, the provider registry and the
    // notification destinations at `Config`, the post-processing steps and upload destinations
    // at `Queue`. Announcing only on the administration channel left a task-scoped token
    // reading a stale list it was never told had changed — invisible from the interface,
    // because a browser session holds every scope.
    let payload = serde_json::json!({ "resource": "plugin", "plugin_id": id, "action": action });
    for kind in [
        rd_core::EventKind::PluginChanged,
        rd_core::EventKind::PluginCatalogChanged,
        rd_core::EventKind::PostprocessCatalogChanged,
    ] {
        state
            .database
            .broadcast(rd_core::EventEnvelope::new(kind, payload.clone()));
    }
}

/// Body of `PATCH /api/v1/plugins/{id}`.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct PluginEnabledRequest {
    pub enabled: bool,
}

#[utoipa::path(
    patch,
    path = "/api/v1/plugins/{id}",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    request_body = PluginEnabledRequest,
    responses((status = 200, body = MessageResponse))
)]
/// Switches one installed plugin off or back on.
///
/// The plugin stays installed and keeps being listed — otherwise it could not be switched back
/// on — but it is no longer loaded, compiled or executed. Like installing one, this takes full
/// effect on the next start, because resolvers and extension hosts are built when their
/// subsystem starts.
pub async fn set_plugin_enabled(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PluginEnabledRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let mut settings = crate::handlers::stored_settings(&state.database).await?;
    settings.disabled_plugins.retain(|entry| entry != &id);
    if !request.enabled {
        settings.disabled_plugins.push(id.clone());
    }
    // Persisted directly rather than through `apply_settings`: that fans the whole document out
    // to the scheduler, bandwidth, power and the auth service, and on an installation that never
    // saved its settings it would write every default over the running configuration. Nothing in
    // that fan-out reads this list — it is applied when the plugin subsystems start.
    state
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    state.plugins.set_disabled(settings.disabled_plugins);
    // A switched-off plugin contributes no provider, the same way startup filters it out. Doing
    // it here too means the accounts list stops offering it at once instead of at the next
    // start — and switching it back on brings it straight back.
    state.plugins.refresh_providers().await;
    announce_plugin(
        &state,
        &id,
        if request.enabled {
            "enabled"
        } else {
            "disabled"
        },
    );
    Ok(Json(if request.enabled {
        MessageResponse::new("plugin.enabled", "Plugin switched on. Restart to apply.")
    } else {
        MessageResponse::new("plugin.disabled", "Plugin switched off. Restart to apply.")
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/plugins/install",
    tag = "plugins",
    params(InstallPluginQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = MessageResponse),
        (status = 409, description = "Signed by a key the user has not confirmed yet", body = MessageResponse)
    )
)]
pub async fn install_plugin(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Query(query): Query<InstallPluginQuery>,
    bytes: Bytes,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_PACKAGE_BYTES {
        return Err(ApiError::bad_request(
            "plugin.package_size_invalid",
            format!(
                "The .rdplug package must be between 1 byte and {MAX_PLUGIN_PACKAGE_BYTES} bytes"
            ),
        )
        .with_param("max", MAX_PLUGIN_PACKAGE_BYTES));
    }
    // A package signed by an unknown author is refused once, reporting the fingerprint. The
    // client shows it, and re-sends the same bytes with the fingerprint it displayed; only an
    // exact match confirms the key, so the user always approves the key they actually saw.
    if let Some(confirmed) = query.trust_fingerprint.as_deref() {
        confirm_signing_key(&state, &bytes, confirmed).await?;
    }
    let installed = state
        .plugins
        .install_bytes(bytes.to_vec())
        .await
        .map_err(install_error)?;

    // The provider row is live at once, so an account can be created straight away; the
    // resolver itself starts working after a restart, since resolvers are built when the
    // scheduler starts. A transfer backend contributes no row — it serves URL schemes, not an
    // account — so there is nothing to register and nothing that can clash.
    if let Some(row) = rd_plugin_host::provider_spec_from_manifest(&installed.manifest)
        && let Err(error) = rd_provider_registry::try_register_dynamic(row)
    {
        // A plugin whose provider cannot be registered would sit on disk doing nothing and
        // still show up as installed, so undo the write before reporting the conflict.
        if let Err(cleanup) = state.plugins.remove_installed(&installed.path).await {
            tracing::warn!(
                path = %installed.path.display(),
                error = %cleanup,
                "could not roll back a plugin whose provider was rejected"
            );
        }
        return Err(ApiError::bad_request(
            "plugin.provider_slug_taken",
            format!("Plugin provider could not be registered: {error}"),
        )
        .with_param("slug", installed.manifest.message_slug().to_owned()));
    }

    // The whole set is rebuilt rather than added to: a plugin declaring a secret fragment
    // host is live immediately, for the same reason its provider row is (RD-110-38).
    state.plugins.refresh_providers().await;

    announce_plugin(&state, &installed.manifest.id.to_string(), "installed");
    // A trust decision, and the sharpest one this service offers: from here on, code somebody
    // else wrote runs inside it. The record names the plugin, the version and — when the key
    // was confirmed in this very request — the fingerprint the person approved.
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::PluginInstalled)
        .by(&audit)
        .target("plugin", installed.manifest.id)
        .named(installed.manifest.name.clone())
        .detail("version", &installed.manifest.version);
    if let Some(fingerprint) = query.trust_fingerprint.as_deref() {
        event = event.detail("confirmed_key", fingerprint);
    }
    crate::audit::record(&state, event).await;
    let path = installed.path.display().to_string();
    Ok((
        StatusCode::CREATED,
        Json(
            MessageResponse::new(
                "plugin.installed_restart_required",
                format!("Plugin installed at {path}; restart to activate it"),
            )
            .with_param("path", path),
        ),
    ))
}

/// Verifies that `bytes` really is signed by the key whose fingerprint the user confirmed,
/// then records it so the package still verifies after a restart.
async fn confirm_signing_key(
    state: &AppState,
    bytes: &Bytes,
    confirmed: &str,
) -> Result<(), ApiError> {
    let verifier = state.plugins.verifier().clone();
    let payload = bytes.to_vec();
    let outcome = tokio::task::spawn_blocking(move || verifier.verify_bytes(&payload))
        .await
        .map_err(|error| {
            ApiError::bad_request(
                "plugin.install_failed",
                format!("Plugin verification did not complete: {error}"),
            )
        })?;
    let Err(VerifyError::UntrustedKey {
        key_id,
        public_key,
        fingerprint,
        name,
        ..
    }) = outcome
    else {
        // Already trusted, or broken for an unrelated reason: fall through and let the
        // install attempt report the real outcome.
        return Ok(());
    };
    if !fingerprint.eq_ignore_ascii_case(confirmed.trim()) {
        return Err(ApiError::bad_request(
            "plugin.key_fingerprint_mismatch",
            "The confirmed fingerprint does not match the package's signing key",
        ));
    }
    state
        .plugins
        .verifier()
        .trust_key_base64(key_id.clone(), &public_key)
        .map_err(|error| {
            ApiError::bad_request(
                "plugin.install_failed",
                format!("Signing key could not be trusted: {error}"),
            )
        })?;
    state
        .database
        .trust_plugin_key(rd_db::NewPluginTrustedKey {
            key_id,
            public_key,
            fingerprint,
            plugin_name: Some(name),
        })
        .await?;
    Ok(())
}

fn install_error(error: VerifyError) -> ApiError {
    match error {
        VerifyError::UntrustedKey {
            key_id,
            fingerprint,
            name,
            version,
            ..
        } => ApiError::conflict(
            "plugin.key_untrusted",
            format!("{name} {version} is signed by a key you have not confirmed yet"),
        )
        .with_param("key_id", key_id)
        .with_param("fingerprint", fingerprint)
        .with_param("name", name)
        .with_param("version", version),
        VerifyError::Other(error) => ApiError::bad_request(
            "plugin.install_failed",
            format!("Plugin could not be installed: {error}"),
        )
        .with_param("reason", error.to_string()),
    }
}

#[utoipa::path(get, path = "/api/v1/plugins/keys", tag = "plugins", responses((status = 200, body = [PluginTrustedKeyResponse])))]
pub async fn list_plugin_keys(
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginTrustedKeyResponse>>, ApiError> {
    let keys = state
        .database
        .list_plugin_trusted_keys()
        .await?
        .into_iter()
        .map(PluginTrustedKeyResponse::from)
        .collect();
    Ok(Json(keys))
}

#[utoipa::path(
    delete,
    path = "/api/v1/plugins/keys/{key_id}",
    tag = "plugins",
    params(("key_id" = String, Path,)),
    responses((status = 200, body = MessageResponse))
)]
pub async fn revoke_plugin_key(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(key_id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    // Dropping it from the live verifier stops new installs at once; plugins already
    // installed keep running until the next start, where re-verification skips them.
    state.plugins.verifier().revoke_key(&key_id).ok();
    state.database.revoke_plugin_key(key_id.clone()).await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginKeyRevoked)
            .by(&audit)
            .target("plugin_key", key_id.clone()),
    )
    .await;
    Ok(Json(
        MessageResponse::new(
            "plugin.key_revoked",
            "Signing key revoked; plugins signed with it are skipped after a restart",
        )
        .with_param("key_id", key_id),
    ))
}

/// Which package to withdraw: the digest itself, or the installed version to hash.
///
/// Both spellings exist because both questions are real. An operator acting on an advisory has
/// a digest and may not have the package installed at all; an operator acting on what the
/// plugin manager shows has an id and a version and no way to compute a digest by hand.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PluginDigestRevocationRequest {
    /// The content digest, 64 hex characters. Wins over `plugin_id`/`version` when both are sent.
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub plugin_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Why, in the operator's own words. Kept verbatim and shown beside the entry.
    #[serde(default)]
    pub reason: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/plugins/revocations", tag = "plugins", responses((status = 200, body = [PluginDigestRevocationResponse])))]
pub async fn list_plugin_revocations(
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginDigestRevocationResponse>>, ApiError> {
    let revocations = state
        .database
        .list_plugin_digest_revocations()
        .await?
        .into_iter()
        .map(PluginDigestRevocationResponse::from)
        .collect();
    Ok(Json(revocations))
}

#[utoipa::path(
    post,
    path = "/api/v1/plugins/revocations",
    tag = "plugins",
    request_body = PluginDigestRevocationRequest,
    responses((status = 200, body = MessageResponse), (status = 400), (status = 404))
)]
pub async fn revoke_plugin_digest(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<PluginDigestRevocationRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned);
    let target = resolve_digest(&state, request).await?;
    let hex = rd_plugin_host::format_package_digest(&target.digest);
    // The row first: it is what the next start reads back, and a live set that outlives the
    // record it was meant to mirror is the one inconsistency a restart cannot correct.
    state
        .database
        .revoke_plugin_digest(rd_db::NewPluginDigestRevocation {
            digest: hex.clone(),
            plugin_id: target.plugin_id,
            plugin_name: target.plugin_name,
            version: target.version,
            reason,
        })
        .await?;
    // Nothing already running is torn down; the refusal takes effect at the next load, exactly
    // as a revoked signing key does.
    let newly = state
        .plugins
        .verifier()
        .revoke_package_digest(target.digest)
        .unwrap_or(true);
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginDigestRevoked)
            .by(&audit)
            // A package digest is a public identifier of a build, not a credential.
            .target("plugin_digest", hex.clone())
            .detail("newly_revoked", newly),
    )
    .await;
    Ok(Json(
        if newly {
            MessageResponse::new(
                "plugin.digest_revoked",
                "Package withdrawn; it is refused the next time plugins are loaded",
            )
        } else {
            MessageResponse::new(
                "plugin.digest_already_revoked",
                "That package was already withdrawn",
            )
        }
        .with_param("digest", hex),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/plugins/revocations/{digest}",
    tag = "plugins",
    params(("digest" = String, Path,)),
    responses((status = 200, body = MessageResponse), (status = 400), (status = 404))
)]
pub async fn unrevoke_plugin_digest(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(digest): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let raw = rd_plugin_host::parse_package_digest(&digest).map_err(|_| invalid_digest())?;
    let hex = rd_plugin_host::format_package_digest(&raw);
    let removed = state.database.unrevoke_plugin_digest(hex.clone()).await?;
    if !removed {
        return Err(ApiError::not_found(
            "plugin.digest_not_revoked",
            "That package is not withdrawn",
        )
        .with_param("digest", hex));
    }
    state.plugins.verifier().unrevoke_package_digest(&raw).ok();
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginDigestUnrevoked)
            .by(&audit)
            .target("plugin_digest", hex.clone()),
    )
    .await;
    Ok(Json(
        MessageResponse::new(
            "plugin.digest_unrevoked",
            "Withdrawal lifted; the package is accepted again the next time plugins are loaded",
        )
        .with_param("digest", hex),
    ))
}

/// What a withdrawal names: the digest that decides, plus the context that describes it.
///
/// The context is not identity — the digest alone decides what is refused — but without it the
/// plugin manager could only name the withdrawn version by re-hashing every installed
/// component on every page load.
struct WithdrawalTarget {
    digest: [u8; 32],
    plugin_id: Option<String>,
    plugin_name: Option<String>,
    version: Option<String>,
}

async fn resolve_digest(
    state: &AppState,
    request: PluginDigestRevocationRequest,
) -> Result<WithdrawalTarget, ApiError> {
    if let Some(text) = request.digest.as_deref() {
        let digest = rd_plugin_host::parse_package_digest(text).map_err(|_| invalid_digest())?;
        return Ok(WithdrawalTarget {
            digest,
            plugin_id: request.plugin_id,
            plugin_name: None,
            version: request.version,
        });
    }
    let (Some(id), Some(version)) = (request.plugin_id, request.version) else {
        return Err(invalid_digest());
    };
    let digest = state
        .plugins
        .installed_package_digest(&id, &version)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "plugin.version_not_installed",
                "That plugin version is not installed here",
            )
            .with_param("plugin_id", id.clone())
            .with_param("version", version.clone())
        })?;
    let name = state
        .plugins
        .list_installed()
        .await?
        .into_iter()
        .map(InstalledPluginResponse::from)
        .find(|plugin| plugin.id.to_string() == id && plugin.version == version)
        .map(|plugin| plugin.name);
    Ok(WithdrawalTarget {
        digest,
        plugin_id: Some(id),
        plugin_name: name,
        version: Some(version),
    })
}

fn invalid_digest() -> ApiError {
    ApiError::bad_request(
        "plugin.digest_invalid",
        "A withdrawal names either a 64-character package digest or an installed id and version",
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/plugins/i18n/{locale}",
    tag = "plugins",
    params(("locale" = String, Path,)),
    responses((status = 200, body = serde_json::Value))
)]
pub async fn plugin_messages(
    State(state): State<AppState>,
    Path(locale): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !rd_plugin_host::valid_language(&locale) {
        return Err(ApiError::bad_request(
            "plugin.locale_invalid",
            "Locale must be a two-letter language tag",
        ));
    }
    let bundle = state.plugins.locale_bundle(locale).await?;
    Ok(Json(bundle))
}

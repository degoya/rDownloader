//! Trusted signing keys and revoked package digests.

use super::*;

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
    // The row first, as with a digest withdrawal: it is what the next start reads back, and a
    // live verifier that has dropped a key the database still trusts would quietly trust it
    // again after a restart.
    state.database.revoke_plugin_key(key_id.clone()).await?;
    // Dropping it from the live verifier stops new installs at once; plugins already
    // installed keep running until the next start, where re-verification skips them.
    if let Err(error) = state.plugins.verifier().revoke_key(&key_id) {
        tracing::warn!(
            key_id = %key_id,
            error = %format!("{error:#}"),
            "signing key revoked in the database but not in the live verifier; it takes effect at the next start"
        );
    }
    // Plugins signed with it are skipped from the next start (RD-1240-32).
    state.restart.record(crate::dto::RestartReason {
        code: "plugin_key_revoked".to_owned(),
        plugin_id: None,
        name: Some(key_id.clone()),
        version: None,
        from_version: None,
    });
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
    // Refused from the next start (RD-1240-32), recorded once the withdrawal is new.
    let pending = crate::dto::RestartReason {
        code: "plugin_digest_revoked".to_owned(),
        plugin_id: target.plugin_id.clone(),
        name: target.plugin_name.clone(),
        version: target.version.clone(),
        from_version: None,
    };
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
    let newly = match state
        .plugins
        .verifier()
        .revoke_package_digest(target.digest)
    {
        Ok(newly) => newly,
        Err(error) => {
            // The row is written and wins at the next start; only the live set lags behind.
            tracing::warn!(
                digest = %hex,
                error = %format!("{error:#}"),
                "package withdrawn in the database but not in the live verifier; it takes effect at the next start"
            );
            true
        }
    };
    if newly {
        state.restart.record(pending);
    }
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
    if let Err(error) = state.plugins.verifier().unrevoke_package_digest(&raw) {
        tracing::warn!(
            digest = %hex,
            error = %format!("{error:#}"),
            "withdrawal lifted in the database but not in the live verifier; it takes effect at the next start"
        );
    }
    // Accepted again from the next start (RD-1240-32).
    state.restart.record(crate::dto::RestartReason {
        code: "plugin_digest_unrevoked".to_owned(),
        plugin_id: None,
        name: None,
        version: None,
        from_version: None,
    });
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
pub(super) struct WithdrawalTarget {
    pub(super) digest: [u8; 32],
    pub(super) plugin_id: Option<String>,
    pub(super) plugin_name: Option<String>,
    pub(super) version: Option<String>,
}

pub(super) async fn resolve_digest(
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

pub(super) fn invalid_digest() -> ApiError {
    ApiError::bad_request(
        "plugin.digest_invalid",
        "A withdrawal names either a 64-character package digest or an installed id and version",
    )
}

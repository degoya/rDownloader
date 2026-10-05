//! Installing a plugin package: trust on first use, registration and the refusals.

use super::*;

/// Confirmation of the fingerprint the client was shown for an unknown signing key.
#[derive(Deserialize, IntoParams)]
pub struct InstallPluginQuery {
    /// Hex SHA-256 of the package's public key, exactly as the 409 response reported it.
    #[serde(default)]
    pub(super) trust_fingerprint: Option<String>,
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
    headers: axum::http::HeaderMap,
    bytes: Bytes,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
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
    let running = register_installed(&state, &installed).await?;
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
    Ok((
        StatusCode::CREATED,
        Json(installed_message(&installed, running)),
    ))
}

/// What an install answers: running now, or from the next start (RD-170-12).
pub(crate) fn installed_message(
    installed: &rd_plugin_host::InstalledPackage,
    running: bool,
) -> MessageResponse {
    let path = installed.path.display().to_string();
    let message = if running {
        MessageResponse::new(
            "plugin.installed",
            format!("Plugin installed at {path}; it runs now"),
        )
    } else {
        MessageResponse::new(
            "plugin.installed_restart_required",
            format!("Plugin installed at {path}; restart to activate it"),
        )
    };
    message.with_param("path", path)
}

/// Makes a freshly installed package visible: its provider row, the secret fragment hosts, a
/// first install in the running service, and the event every open client reloads on. Shared by
/// the upload, the repository and the bundle, so a package is live in exactly the same ways
/// wherever it came from. Answers whether the plugin runs now; otherwise it runs from the next
/// start.
pub(crate) async fn register_installed(
    state: &AppState,
    installed: &rd_plugin_host::InstalledPackage,
) -> Result<bool, ApiError> {
    // The provider row is live at once, so an account can be created straight away; the
    // plugin itself joins the running service when it is a first install (RD-170-12), and an
    // update runs from the next start. A transfer backend contributes no row — it serves URL
    // schemes, not an account — so there is nothing to register and nothing that can clash.
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
    let running = crate::plugin_live::activate_first_install(state, installed).await;

    announce_plugin(state, &installed.manifest.id.to_string(), "installed");
    Ok(running)
}

/// Verifies that `bytes` really is signed by the key whose fingerprint the user confirmed,
/// then records it so the package still verifies after a restart.
pub(crate) async fn confirm_signing_key(
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

pub(crate) fn install_error(error: VerifyError) -> ApiError {
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

//! The repositories at startup, and the refresh that installs the updates they offer.

use super::*;

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
pub(super) async fn install_offer(
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

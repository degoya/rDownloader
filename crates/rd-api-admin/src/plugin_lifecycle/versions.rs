//! The versions of a plugin: which exist, which is chosen, which runs, and storing the choice.

use super::*;

/// The versions of every installed plugin that the next start can load, by plugin id.
///
/// Read from the manifests on disk, minus every version a withdrawal names: a withdrawn
/// package is refused at load, so it can be neither active nor under test. A version withdrawn
/// by digest alone is not recognised here and is refused by the start instead.
pub(crate) async fn loadable_versions(
    state: &AppState,
) -> Result<BTreeMap<String, Vec<String>>, ApiError> {
    let withdrawn: HashSet<(String, String)> = state
        .database
        .list_plugin_digest_revocations()
        .await?
        .into_iter()
        .filter_map(|entry| Some((entry.plugin_id?, entry.version?)))
        .collect();
    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for manifest in state.plugins.list_installed().await? {
        let id = manifest.id.to_string();
        if withdrawn.contains(&(id.clone(), manifest.version.clone())) {
            continue;
        }
        versions.entry(id).or_default().push(manifest.version);
    }
    Ok(versions)
}

pub(super) fn choice_of(row: &rd_db::PluginVersionChoice) -> VersionChoice {
    VersionChoice {
        active: row.active_version.clone(),
        staged: row.staged_version.clone(),
    }
}

/// The default version of `versions` under `choice`.
pub(crate) fn effective(versions: &[String], choice: Option<&VersionChoice>) -> Option<String> {
    let versions: Vec<&str> = versions.iter().map(String::as_str).collect();
    default_version(&versions, choice).map(str::to_owned)
}

/// The loadable versions of plugin `id` that this start loaded: those already installed when
/// it began (RD-160-09), and a first install that joined the running service (RD-170-12). Any
/// other version installed since lies on disk and runs from the next start. Without a record of
/// the start every loadable version counts, as it did before.
pub(super) fn loaded_at_start(
    started: Option<&StartedVersions>,
    id: &str,
    versions: &[String],
) -> Vec<String> {
    let Some(started) = started else {
        return versions.to_vec();
    };
    let at_start = started.get(id).map(Vec::as_slice).unwrap_or_default();
    versions
        .iter()
        .filter(|version| at_start.contains(version))
        .cloned()
        .collect()
}

/// The version of plugin `id` new work runs on right now: this start's choice over the
/// versions this start loaded.
pub(super) fn running_version(state: &AppState, id: &str, versions: &[String]) -> Option<String> {
    let started = state.plugins.started_versions();
    effective(
        &loaded_at_start(started.as_ref(), id, versions),
        state.plugins.version_choices().get(id),
    )
}

/// Every installed plugin's lifecycle, and the version each runs on now.
pub(crate) async fn lifecycles(
    state: &AppState,
    versions: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<PluginLifecycleResponse>, ApiError> {
    let stored: BTreeMap<String, rd_db::PluginVersionChoice> = state
        .database
        .list_plugin_version_choices()
        .await?
        .into_iter()
        .map(|row| (row.plugin_id.clone(), row))
        .collect();
    let running = state.plugins.version_choices();
    let started = state.plugins.started_versions();
    Ok(versions
        .iter()
        .map(|(id, installed)| {
            let row = stored.get(id);
            let next = effective(installed, row.map(choice_of).as_ref());
            let now = effective(
                &loaded_at_start(started.as_ref(), id, installed),
                running.get(id),
            );
            let staged = row
                .and_then(|row| row.staged_version.clone())
                .filter(|version| installed.contains(version));
            let staged_now = running
                .get(id)
                .and_then(|choice| choice.staged.clone())
                .filter(|version| installed.contains(version));
            PluginLifecycleResponse {
                plugin_id: id.clone(),
                restart_required: next != now || staged != staged_now,
                active_version: next,
                running_version: now,
                staged_version: staged,
                previous_version: row
                    .and_then(|row| row.previous_version.clone())
                    .filter(|version| installed.contains(version)),
                update_policy: row.map_or(PluginUpdatePolicy::Manual, |row| {
                    PluginUpdatePolicy::parse(&row.update_policy)
                }),
            }
        })
        .collect())
}

/// One plugin's stored choice and loadable versions, refused when the plugin is not installed.
pub(crate) async fn current(
    state: &AppState,
    id: &str,
) -> Result<(rd_db::NewPluginVersionChoice, Vec<String>), ApiError> {
    let versions = loadable_versions(state).await?.remove(id).ok_or_else(|| {
        ApiError::not_found("plugin.not_installed", "This plugin is not installed")
    })?;
    let row = state.database.plugin_version_choice(id).await?;
    let choice = rd_db::NewPluginVersionChoice {
        plugin_id: id.to_owned(),
        active_version: row.as_ref().and_then(|row| row.active_version.clone()),
        previous_version: row.as_ref().and_then(|row| row.previous_version.clone()),
        staged_version: row.as_ref().and_then(|row| row.staged_version.clone()),
        update_policy: row.map_or_else(
            || PluginUpdatePolicy::Manual.as_str().to_owned(),
            |row| row.update_policy,
        ),
    };
    Ok((choice, versions))
}

/// The audit record of one version choice. Which version runs is a decision about which code
/// runs, audited like the install that put it on disk; `choice` names the route.
pub(super) fn chosen(audit: &AuditContext, id: &str, choice: &str) -> AuditEvent {
    AuditEvent::success(rd_core::AuditAction::PluginVersionChosen)
        .by(audit)
        .target("plugin", id)
        .detail("choice", choice)
}

pub(crate) fn pointer(choice: &rd_db::NewPluginVersionChoice) -> VersionChoice {
    VersionChoice {
        active: choice.active_version.clone(),
        staged: choice.staged_version.clone(),
    }
}

/// The health check before a version may be pointed at (RD-140-02).
///
/// The version is verified in full, exactly as the next start will load it: signature,
/// withdrawal, manifest, locales and the component's validation. A pointer at a version that
/// fails any of them would only move the plugin to the next version at restart anyway, so the
/// refusal says so now, with the reason.
pub(super) async fn healthy(
    state: &AppState,
    id: &str,
    version: &str,
) -> Result<rd_plugin_host::PluginManifest, ApiError> {
    match state.plugins.verify_installed_version(id, version).await {
        Ok(Some(manifest)) => Ok(manifest),
        Ok(None) => Err(ApiError::not_found(
            "plugin.version_not_installed",
            "That plugin version is not installed here",
        )
        .with_param("plugin_id", id)
        .with_param("version", version)),
        Err(error) => Err(ApiError::conflict(
            "plugin.version_unhealthy",
            format!("This plugin version would not load: {error:#}"),
        )
        .with_param("version", version)
        .with_param("reason", format!("{error:#}"))),
    }
}

pub(crate) async fn save(
    state: &AppState,
    choice: rd_db::NewPluginVersionChoice,
) -> Result<(), ApiError> {
    state.database.save_plugin_version_choice(choice).await?;
    Ok(())
}

/// Drops every pointer at a version that was just removed.
///
/// A pointer at a missing version already means nothing to the next start; clearing it keeps
/// the plugin manager from offering a rollback to, or a test of, a version that is gone.
pub(crate) async fn forget_version(
    state: &AppState,
    id: &str,
    version: &str,
) -> Result<(), ApiError> {
    let Some(row) = state.database.plugin_version_choice(id).await? else {
        return Ok(());
    };
    let keep = |pointer: Option<String>| pointer.filter(|pointer| pointer != version);
    let choice = rd_db::NewPluginVersionChoice {
        plugin_id: row.plugin_id.clone(),
        active_version: keep(row.active_version.clone()),
        previous_version: keep(row.previous_version.clone()),
        staged_version: keep(row.staged_version.clone()),
        update_policy: row.update_policy.clone(),
    };
    if choice.active_version == row.active_version
        && choice.previous_version == row.previous_version
        && choice.staged_version == row.staged_version
    {
        return Ok(());
    }
    save(state, choice).await
}

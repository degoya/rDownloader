//! The plugin boot of `serve`, in the order it runs: the verifier's trust and withdrawals, the
//! installer with its switches and version choices, the bundled packages, and the providers of
//! what is installed.

use std::path::PathBuf;

use anyhow::Result;
use rd_db::Database;

use crate::{ServeArgs, StoredSettings};

/// The plugin installer, with its trust, withdrawals, switches, version choices, the bundled
/// packages and the providers of what is installed.
pub(crate) async fn open_plugins(
    args: &ServeArgs,
    database: &Database,
    stored: &StoredSettings,
) -> Result<rd_plugin_host::PluginInstaller> {
    let plugin_verifier = rd_pack::plugin::build_plugin_verifier(
        args.plugin_development_mode,
        &args.trusted_plugin_keys,
        !args.no_default_plugin_key,
    )?;
    // Keys the user confirmed on first use must be trusted before anything is verified.
    load_trusted_plugin_keys(database, &plugin_verifier).await;
    load_withdrawn_plugin_digests(database, &plugin_verifier).await;
    load_withdrawn_plugin_keys(database, &plugin_verifier).await;
    let plugins = rd_plugin_host::PluginInstaller::new(args.plugin_root.clone(), plugin_verifier);
    // Applied before anything loads plugins: a switched-off plugin must not be compiled or
    // executed, while still being listed by the API so it can be switched back on.
    plugins.set_disabled(stored.disabled_plugins.iter().cloned());
    // Likewise the version choices (RD-140-02): which installed version runs and which one is
    // under test are decided before the first package is loaded, and stay as they were read
    // here until the next start.
    load_plugin_version_choices(database, &plugins).await;
    sync_bundled_plugins(
        database,
        &plugins,
        args.bundled_plugins.clone(),
        args.install_all_bundled_plugins,
    )
    .await;
    // What is on disk now is what this start loads; a version installed from here on runs from
    // the next start, and the plugin manager says so (RD-160-09).
    if let Err(error) = plugins.record_started_versions().await {
        tracing::warn!(%error, "could not record the plugin versions this start loads");
    }
    // Installed manifests contribute their provider rows before the scheduler builds
    // resolvers, so account creation and the HTTP sandbox know about them from the start.
    plugins.refresh_providers().await;
    Ok(plugins)
}

/// Restores the trust-on-first-use keys so installed third-party plugins still verify.
async fn load_trusted_plugin_keys(database: &Database, verifier: &rd_plugin_host::PluginVerifier) {
    let keys = match database.list_plugin_trusted_keys().await {
        Ok(keys) => keys,
        Err(error) => {
            tracing::warn!(error = %error, "could not read confirmed plugin signing keys");
            return;
        }
    };
    for key in keys {
        if let Err(error) = verifier.trust_key_base64(key.key_id.clone(), &key.public_key) {
            tracing::warn!(
                key_id = %key.key_id,
                error = %error,
                "skipping unreadable confirmed plugin signing key"
            );
        }
    }
}

/// Seeds the plugin installer with the stored version choices (RD-140-02).
///
/// A choice that cannot be read costs the choice, not the start: every plugin then runs its
/// newest version, which is what it did before choices existed, and the failure is said out
/// loud.
async fn load_plugin_version_choices(
    database: &Database,
    plugins: &rd_plugin_host::PluginInstaller,
) {
    match database.list_plugin_version_choices().await {
        Ok(rows) => plugins.set_version_choices(
            rows.into_iter()
                .map(|row| {
                    (
                        row.plugin_id,
                        rd_plugin_host::VersionChoice {
                            active: row.active_version,
                            staged: row.staged_version,
                        },
                    )
                })
                .collect(),
        ),
        Err(error) => {
            tracing::warn!(error = %error, "could not read the plugin version choices");
        }
    }
}

/// Restores the package digests an operator has withdrawn.
///
/// Read before anything loads a plugin: the whole point of a withdrawal is that the package is
/// refused on the *next* load, so a set restored afterwards would first let through exactly the
/// version it names. A row that cannot be read costs that one withdrawal and is said out loud;
/// none of this may keep the service from starting, because a database that cannot be read here
/// is a problem to report, not a reason to leave the person without a download manager.
async fn load_withdrawn_plugin_digests(
    database: &Database,
    verifier: &rd_plugin_host::PluginVerifier,
) {
    let revocations = match database.list_plugin_digest_revocations().await {
        Ok(revocations) => revocations,
        Err(error) => {
            tracing::warn!(error = %error, "could not read withdrawn plugin packages");
            return;
        }
    };
    let mut digests = Vec::with_capacity(revocations.len());
    for revocation in revocations {
        match rd_plugin_host::parse_package_digest(&revocation.digest) {
            Ok(digest) => digests.push(digest),
            Err(error) => tracing::warn!(
                digest = %revocation.digest,
                error = %error,
                "skipping an unreadable withdrawn plugin package"
            ),
        }
    }
    // One call: it replaces the set rather than adding to it.
    if let Err(error) = verifier.set_revoked_package_digests(digests) {
        tracing::warn!(error = %error, "could not apply the withdrawn plugin packages");
    }
}

/// Restores the plugin signing keys a repository index withdrew (RD-140-01).
///
/// Read before anything loads a plugin, for the same reason as the withdrawn digests; a key
/// withdrawn once stays withdrawn even when a later index no longer names it.
async fn load_withdrawn_plugin_keys(
    database: &Database,
    verifier: &rd_plugin_host::PluginVerifier,
) {
    match database.list_plugin_withdrawn_keys().await {
        Ok(keys) => {
            if let Err(error) =
                verifier.set_withdrawn_keys(keys.into_iter().map(|key| key.fingerprint))
            {
                tracing::warn!(error = %error, "could not apply the withdrawn plugin signing keys");
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "could not read withdrawn plugin signing keys");
        }
    }
}

/// Settings key recording that the bundle's first-start install happened (RD-160-05).
const BUNDLED_FIRST_START_SETTING: &str = "plugins.bundled_first_start";

/// Which bundled packages this start installs that are not installed yet (RD-160-05).
///
/// Only the first start of a fresh installation installs anything new, and only the services
/// that need no account; every later start updates what is installed and offers the rest. An
/// installation that already has plugins when this first runs counts as chosen: what is
/// installed stays, and nothing it does not have is added. A marker that cannot be read costs
/// the first-start install, never an unasked one.
async fn bundled_policy(
    database: &Database,
    plugins: &rd_plugin_host::PluginInstaller,
    install_all: bool,
) -> rd_plugin_host::BundledPolicy {
    if install_all {
        return rd_plugin_host::BundledPolicy::All;
    }
    let first_start = match database.get_setting(BUNDLED_FIRST_START_SETTING).await {
        Ok(marker) => marker.is_none(),
        Err(error) => {
            tracing::warn!(%error, "could not read the bundled plugin marker");
            false
        }
    };
    let untouched = plugins
        .list_installed()
        .await
        .is_ok_and(|installed| installed.is_empty())
        && plugins
            .list_incompatible()
            .await
            .is_ok_and(|incompatible| incompatible.is_empty());
    if first_start && untouched {
        rd_plugin_host::BundledPolicy::FirstStart
    } else {
        rd_plugin_host::BundledPolicy::InstalledOnly
    }
}

/// Installs newer bundled `.rdplug` packages from `<exe dir>/plugins` (or the given directory).
async fn sync_bundled_plugins(
    database: &Database,
    plugins: &rd_plugin_host::PluginInstaller,
    directory: Option<PathBuf>,
    install_all: bool,
) {
    let directory = directory.or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("plugins")))
    });
    let Some(directory) = directory else {
        return;
    };
    // An image built without scripts/build-plugins.sh copies an empty directory, and the result
    // is a service with no hoster resolvers and nothing anywhere saying why. A *missing*
    // directory is the ordinary case for a plain binary and stays quiet.
    let listed = directory.clone();
    let empty = tokio::task::spawn_blocking(move || {
        std::fs::read_dir(&listed).is_ok_and(|entries| {
            !entries.flatten().any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rdplug")
            })
        })
    })
    .await
    .unwrap_or(false);
    if empty {
        tracing::warn!(
            directory = %directory.display(),
            "no bundled plugin packages found; hoster resolvers and other plugin providers will be missing"
        );
    }
    let policy = bundled_policy(database, plugins, install_all).await;
    match rd_plugin_host::sync_bundled(plugins, &directory, policy).await {
        Ok(report) => {
            for (id, version) in &report.installed {
                tracing::info!(%id, version, "installed bundled plugin");
            }
            for (name, reason) in &report.rejected {
                tracing::warn!(package = name, reason, "bundled plugin rejected");
            }
            if report.available > 0 {
                tracing::info!(
                    count = report.available,
                    "bundled plugins not installed; offered as available in the plugin manager"
                );
            }
            if let Err(error) = database
                .set_setting(
                    BUNDLED_FIRST_START_SETTING.to_owned(),
                    serde_json::Value::Bool(true),
                )
                .await
            {
                tracing::warn!(%error, "could not record the bundled plugin marker");
            }
        }
        Err(error) => {
            tracing::warn!(%error, directory = %directory.display(), "bundled plugin sync failed")
        }
    }
}

#[cfg(test)]
#[path = "bundled_policy_tests.rs"]
mod bundled_policy_tests;

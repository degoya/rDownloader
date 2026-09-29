//! A first install runs at once (RD-170-12).
//!
//! Resolvers and extension hosts are built once per start. Until 1.7 that meant every install
//! ran from the next start — and the setup wizard installs its services while the service runs,
//! so the accounts step right after it offered DDownload, took the account, and its check found
//! no resolver. A plugin none of whose versions this start loaded now joins the running service
//! the way a start loads it: verified again from disk, instantiated, added to the running set,
//! and recorded as running. Resolvers and the sign-in plugins (`auth`, `oauth`) join — what an
//! account needs. Every other type, and every update of a plugin that already runs, keeps the
//! rule it had: from the next start, because a download that began on one version must not meet
//! another half-way.

use rd_plugin_host::{InstalledPackage, PluginType, PluginTypeRegistry};

use crate::AppState;

/// Brings a freshly installed package into the running service when it is a first install of
/// a type that can join, and answers whether it runs now. `false` means it runs from the next
/// start, which is what the install response then says.
pub(crate) async fn activate_first_install(state: &AppState, installed: &InstalledPackage) -> bool {
    let manifest = &installed.manifest;
    let id = manifest.id;
    let resolvers = state.scheduler.resolvers();
    let live = match manifest.plugin_type {
        PluginType::Resolver | PluginType::Auth | PluginType::OAuth => {
            !loaded(state, installed) && join(state, installed).await
        }
        _ => false,
    };
    if live {
        state
            .plugins
            .record_started_version(&id.to_string(), &manifest.version);
    } else if manifest.plugin_type == PluginType::Resolver {
        resolvers.mark_waiting(id);
    }
    live
}

/// Whether this start already runs some version of the plugin: it was installed when the start
/// loaded, or a version of it joined since.
fn loaded(state: &AppState, installed: &InstalledPackage) -> bool {
    let id = installed.manifest.id;
    state
        .plugins
        .started_versions()
        .is_some_and(|started| started.contains_key(&id.to_string()))
        || state.scheduler.resolvers().has_plugin(id)
}

async fn join(state: &AppState, installed: &InstalledPackage) -> bool {
    let manifest = &installed.manifest;
    let id = manifest.id;
    let package = match state
        .plugins
        .load_installed_version(&id.to_string(), &manifest.version)
        .await
    {
        Ok(Some(package)) => package,
        Ok(None) => return false,
        Err(error) => {
            tracing::warn!(
                plugin = %manifest.name,
                version = %manifest.version,
                %error,
                "a new plugin could not be loaded; it runs from the next start"
            );
            return false;
        }
    };
    let registry = PluginTypeRegistry::new(vec![package]);
    if manifest.plugin_type == PluginType::Resolver {
        let resolvers = state.scheduler.resolvers();
        // Compiling is blocking work; the running chain is swapped once it is done.
        return tokio::task::spawn_blocking(move || resolvers.activate_first_install(&registry))
            .await
            .is_ok_and(|joined| joined > 0);
    }
    state.auth_flows.activate_first_install(registry, id).await
}

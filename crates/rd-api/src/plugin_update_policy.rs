//! Where plugin repositories meet the version manager (RD-140-01, RD-140-02).
//!
//! The repository refresh asks the version manager's store whether a plugin updates itself, and
//! an update it installs moves the version pointers the way the plugin manager would: a plugin
//! that ran its newest version keeps running its newest one, a plugin held back stays held back.

use rd_plugin_host::repository::{UpdatePolicy, UpdatePolicySource};

use crate::{
    ApiError, AppState,
    plugin_lifecycle::{PluginUpdatePolicy, current, effective, pointer, save},
};

/// The per-plugin update policy stored in `plugin_version_choices`, as the repository refresh
/// reads it. The service hands it to `PluginRepositoryService::set_update_policy` at start.
pub struct VersionChoicePolicy(rd_db::Database);

impl VersionChoicePolicy {
    #[must_use]
    pub fn new(database: rd_db::Database) -> Self {
        Self(database)
    }
}

#[async_trait::async_trait]
impl UpdatePolicySource for VersionChoicePolicy {
    async fn policy(&self, plugin_id: &str) -> UpdatePolicy {
        match self.0.plugin_version_choice(plugin_id).await {
            Ok(Some(row))
                if PluginUpdatePolicy::parse(&row.update_policy)
                    == PluginUpdatePolicy::Automatic =>
            {
                UpdatePolicy::Automatic
            }
            Ok(_) => UpdatePolicy::Manual,
            // Manual is the safe reading: nothing installs that nobody asked for.
            Err(error) => {
                tracing::warn!(%error, plugin_id, "could not read a plugin's update policy");
                UpdatePolicy::Manual
            }
        }
    }
}

/// Moves the version pointers onto `version`, which a repository just installed.
///
/// Stored like every other choice, so it takes effect at the next start.
pub(crate) async fn follow_update(
    state: &AppState,
    id: &str,
    version: &str,
) -> Result<(), ApiError> {
    let (choice, installed) = current(state, id).await?;
    match followed(choice, &installed, version) {
        Some(choice) => save(state, choice).await,
        None => Ok(()),
    }
}

/// The choice after `version` was installed next to `installed`, or `None` when nothing moves.
///
/// Only a version newer than every other one moves anything. When the plugin ran its newest
/// version before, the update becomes the one the next start runs and the version it replaces
/// the rollback target. Without a stored active version the newest wins anyway, so only the
/// rollback target is recorded. A plugin held below its newest version on purpose — rolled
/// back, an older version activated, a newer one under test — keeps what it runs; the update
/// waits beside it to be activated or tested.
fn followed(
    mut choice: rd_db::NewPluginVersionChoice,
    installed: &[String],
    version: &str,
) -> Option<rd_db::NewPluginVersionChoice> {
    if effective(installed, None).as_deref() != Some(version) {
        return None;
    }
    let before: Vec<String> = installed
        .iter()
        .filter(|other| *other != version)
        .cloned()
        .collect();
    let replaced = effective(&before, Some(&pointer(&choice)));
    if replaced.is_none() || replaced != effective(&before, None) {
        return None;
    }
    if choice.active_version.is_some() {
        choice.active_version = Some(version.to_owned());
    }
    choice.previous_version = replaced;
    Some(choice)
}

#[cfg(test)]
mod tests {
    use super::followed;

    fn choice(active: Option<&str>, staged: Option<&str>) -> rd_db::NewPluginVersionChoice {
        rd_db::NewPluginVersionChoice {
            plugin_id: "plugin".to_owned(),
            active_version: active.map(str::to_owned),
            previous_version: None,
            staged_version: staged.map(str::to_owned),
            update_policy: "automatic".to_owned(),
        }
    }

    fn installed(versions: &[&str]) -> Vec<String> {
        versions
            .iter()
            .map(|version| (*version).to_owned())
            .collect()
    }

    #[test]
    fn an_update_of_a_plugin_that_followed_its_newest_version_becomes_active() {
        let moved = followed(
            choice(Some("2.0.0"), None),
            &installed(&["1.0.0", "2.0.0", "3.0.0"]),
            "3.0.0",
        )
        .expect("the pointers move");
        assert_eq!(moved.active_version.as_deref(), Some("3.0.0"));
        assert_eq!(moved.previous_version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn without_an_active_version_only_the_rollback_target_is_recorded() {
        let moved = followed(choice(None, None), &installed(&["1.0.0", "2.0.0"]), "2.0.0")
            .expect("the rollback target is recorded");
        assert_eq!(moved.active_version, None);
        assert_eq!(moved.previous_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn a_plugin_held_back_keeps_what_it_runs() {
        // Rolled back from 2.0.0 to 1.0.0.
        let installed_versions = installed(&["1.0.0", "2.0.0", "3.0.0"]);
        assert!(followed(choice(Some("1.0.0"), None), &installed_versions, "3.0.0").is_none());
        // 2.0.0 under test next to the active 1.0.0.
        assert!(
            followed(
                choice(Some("1.0.0"), Some("2.0.0")),
                &installed_versions,
                "3.0.0"
            )
            .is_none()
        );
    }

    #[test]
    fn a_first_install_or_an_older_version_moves_nothing() {
        assert!(followed(choice(None, None), &installed(&["1.0.0"]), "1.0.0").is_none());
        assert!(
            followed(
                choice(Some("2.0.0"), None),
                &installed(&["1.0.0", "1.5.0", "2.0.0"]),
                "1.5.0"
            )
            .is_none()
        );
    }
}

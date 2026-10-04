//! Where plugin repositories meet the version manager (RD-140-01, RD-140-02).
//!
//! The repository refresh asks the version manager's store whether a plugin updates itself, and
//! an update it installs moves the version pointers the way the plugin manager would: a plugin
//! that ran its newest version keeps running its newest one, a plugin held back stays held back.
//!
//! One switch sits above the per-plugin policies (RD-191-10): with it on, every installed plugin
//! updates as if it were set to automatic, plugins installed later included. It never writes
//! the per-plugin policies, so switching it off gives every plugin back the policy it had. The
//! rules that keep an update waiting for a click stay as they are: one that asks for a new
//! permission, and one for a plugin held below its newest version on purpose.

use axum::{Json, extract::State};
use rd_plugin_host::repository::{UpdatePolicy, UpdatePolicySource};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    audit::{AuditContext, AuditEvent},
    dto::MessageResponse,
    plugin_lifecycle::{PluginUpdatePolicy, current, effective, pointer, save},
};

/// The settings key of the switch that sets every installed plugin to automatic updates.
pub const AUTOMATIC_UPDATES_KEY: &str = "plugin_updates";

/// Whether every installed plugin updates itself, whatever its own policy. Off when nothing
/// was ever stored.
pub async fn automatic_for_all(database: &rd_db::Database) -> anyhow::Result<bool> {
    Ok(switched_on(
        database.get_setting(AUTOMATIC_UPDATES_KEY).await?.as_ref(),
    ))
}

/// Reads the stored switch; anything but an explicit `true` is off.
fn switched_on(stored: Option<&serde_json::Value>) -> bool {
    stored
        .and_then(|value| value.get("automatic")?.as_bool())
        .unwrap_or(false)
}

/// The policy one plugin updates by: automatic while the switch for all plugins is on,
/// otherwise its own stored policy, manual without one.
fn policy_of(automatic_for_all: bool, own: Option<&str>) -> UpdatePolicy {
    if automatic_for_all
        || own.map(PluginUpdatePolicy::parse) == Some(PluginUpdatePolicy::Automatic)
    {
        UpdatePolicy::Automatic
    } else {
        UpdatePolicy::Manual
    }
}

/// The per-plugin update policy stored in `plugin_version_choices`, and the switch above it, as
/// the repository refresh reads them. The service hands it to
/// `PluginRepositoryService::set_update_policy` at start.
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
        // Off is the safe reading of a switch that cannot be read: the plugin's own policy
        // still applies.
        let for_all = automatic_for_all(&self.0).await.unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the update switch for all plugins");
            false
        });
        match self.0.plugin_version_choice(plugin_id).await {
            Ok(row) => policy_of(for_all, row.as_ref().map(|row| row.update_policy.as_str())),
            // Manual is the safe reading: nothing installs that nobody asked for.
            Err(error) => {
                tracing::warn!(%error, plugin_id, "could not read a plugin's update policy");
                policy_of(for_all, None)
            }
        }
    }
}

/// Whether every installed plugin updates itself (RD-191-10).
#[derive(Serialize, ToSchema)]
pub struct PluginUpdateSettingsResponse {
    /// On: every installed plugin, and every plugin installed later, updates as if set to
    /// automatic. Off: each plugin's own policy applies. Either way an update that asks for a
    /// new permission waits for a click, and an installed update runs from the next start.
    pub automatic_updates: bool,
}

/// Body of `PUT /api/v1/plugins/updates/settings`.
#[derive(Deserialize, ToSchema)]
pub struct PluginUpdateSettingsRequest {
    pub automatic_updates: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/plugins/updates/settings",
    tag = "plugins",
    responses((status = 200, body = PluginUpdateSettingsResponse))
)]
pub async fn get_plugin_update_settings(
    State(state): State<AppState>,
) -> Result<Json<PluginUpdateSettingsResponse>, ApiError> {
    Ok(Json(PluginUpdateSettingsResponse {
        automatic_updates: automatic_for_all(&state.database).await?,
    }))
}

/// Switches automatic updates for every installed plugin on or off.
///
/// The per-plugin policies are left as they are; the repository refresh reads this switch
/// first, the next refresh installs what it now allows.
#[utoipa::path(
    put,
    path = "/api/v1/plugins/updates/settings",
    tag = "plugins",
    request_body = PluginUpdateSettingsRequest,
    responses((status = 200, body = PluginUpdateSettingsResponse), (status = 400, body = MessageResponse))
)]
pub async fn set_plugin_update_settings(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<PluginUpdateSettingsRequest>,
) -> Result<Json<PluginUpdateSettingsResponse>, ApiError> {
    state
        .database
        .set_setting(
            AUTOMATIC_UPDATES_KEY.to_owned(),
            serde_json::json!({ "automatic": request.automatic_updates }),
        )
        .await?;
    // Code that installs itself is a decision worth a record, like each plugin's own policy.
    crate::audit::record(
        &state,
        AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(&audit)
            .target("plugin_updates", "automatic")
            .detail("automatic_updates", request.automatic_updates),
    )
    .await;
    // Every plugin's card and the update list read the switch; open pages reload on this.
    state.database.broadcast(rd_core::EventEnvelope::new(
        rd_core::EventKind::PluginChanged,
        serde_json::json!({ "resource": "plugin_updates", "action": "automatic" }),
    ));
    Ok(Json(PluginUpdateSettingsResponse {
        automatic_updates: request.automatic_updates,
    }))
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
    use rd_plugin_host::repository::UpdatePolicy;

    use super::{followed, policy_of, switched_on};

    #[test]
    fn the_switch_for_all_plugins_reads_off_unless_it_says_on() {
        assert!(!switched_on(None));
        assert!(!switched_on(Some(&serde_json::json!({}))));
        assert!(!switched_on(Some(
            &serde_json::json!({ "automatic": "true" })
        )));
        assert!(!switched_on(Some(
            &serde_json::json!({ "automatic": false })
        )));
        assert!(switched_on(Some(&serde_json::json!({ "automatic": true }))));
    }

    #[test]
    fn the_switch_for_all_plugins_overrides_a_manual_policy_and_never_the_other_way() {
        assert_eq!(policy_of(true, Some("manual")), UpdatePolicy::Automatic);
        assert_eq!(policy_of(true, None), UpdatePolicy::Automatic);
        assert_eq!(policy_of(false, Some("automatic")), UpdatePolicy::Automatic);
        assert_eq!(policy_of(false, Some("manual")), UpdatePolicy::Manual);
        assert_eq!(policy_of(false, None), UpdatePolicy::Manual);
    }

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

//! Notification destination plugins (RD-090-15).
//!
//! A plugin delivers; it does not decide whether to try again. Retries, backoff and quiet
//! hours stay with the notification hub, which already owns them for the built-in webhook,
//! SMTP and Apprise targets — a destination that could set its own retry policy would be a
//! way for one plugin to keep the queue busy for everyone.

use anyhow::Result;
use rd_plugin_host::{SettingManifest, extension::NotifierPlugin};

use crate::PluginSet;

/// The installed notification destinations, newest version of each, looked up by plugin id —
/// which is what a notification target stores.
///
/// A broken plugin costs its own targets and nothing else: the built-in kinds keep delivering,
/// and the failure is logged rather than taking the hub down with it.
pub type NotifierPlugins = PluginSet<NotifierPlugin>;

/// What a destination looks like to whoever is choosing one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DestinationInfo {
    pub plugin_id: String,
    /// Default display name; the localised one comes from the plugin's own locale bundle.
    pub name: String,
    pub version: String,
    /// The message namespace, which the settings' labels are codes in.
    pub slug: String,
    /// What a target of this destination may be set to (RD-170-09).
    pub settings: Vec<SettingManifest>,
}

impl NotifierPlugins {
    /// Whether a destination would be refused at delivery (RD-130-15), for refusing it when
    /// the target is saved. `None` means the plugin is not installed.
    #[must_use]
    pub fn check_destination(
        &self,
        plugin_id: &str,
        destination: &str,
    ) -> Option<Result<(), rd_core::Failure>> {
        Some(self.get(plugin_id)?.plugin.check_destination(destination))
    }

    /// Whether a target's settings are ones the destination offers (RD-170-09), for refusing
    /// them when the target is saved. `None` means the plugin is not installed.
    #[must_use]
    pub fn check_settings(
        &self,
        plugin_id: &str,
        settings: &[(String, String)],
    ) -> Option<Result<(), rd_core::Failure>> {
        Some(self.get(plugin_id)?.plugin.check_settings(settings))
    }

    /// Every installed destination, sorted by name so the list does not reshuffle itself.
    #[must_use]
    pub fn list(&self) -> Vec<DestinationInfo> {
        self.by_name()
            .into_iter()
            .map(|destination| DestinationInfo {
                plugin_id: destination.manifest.id.to_string(),
                name: destination.manifest.name.clone(),
                version: destination.manifest.version.clone(),
                slug: destination.manifest.message_slug().to_owned(),
                settings: destination
                    .manifest
                    .extension
                    .as_ref()
                    .map(|extension| extension.settings.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Delivers one message through one plugin.
    ///
    /// `Ok(None)` means the plugin is not installed — a target naming a plugin that has been
    /// removed, which the caller reports as a permanent failure rather than retrying forever.
    pub async fn deliver(
        &self,
        plugin_id: &str,
        message: rd_plugin_host::extension::Delivery<'_>,
    ) -> Option<Result<()>> {
        let destination = self.get(plugin_id)?;
        Some(destination.plugin.deliver(message).await)
    }
}

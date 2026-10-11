//! The update settings, as the service's settings document stores them.
//!
//! Seven fields of the `service.settings` blob (`update_check_enabled`, `update_channel`,
//! `update_check_interval_hours`, the automatic install's `update_auto_install` and
//! `update_auto_install_window`, RD-1240-27, the automatic restart's `restart_when_needed`,
//! RD-1240-32, which waits for the same window, and `update_backup_retention_days`, RD-1240-34),
//! read as one slice through
//! `Database::service_settings_or_default` like every other layer's. The settings DTO validates
//! them when they are saved; this reads them and falls back to the defaults for anything out of
//! range, so a broken value never stops the check loop.

use std::ops::RangeInclusive;

use serde::Deserialize;

use crate::{auto_install::InstallWindow, manifest::Channel, offer::parse_version};

/// Hours between two automatic checks when nobody chose otherwise.
pub const DEFAULT_INTERVAL_HOURS: u32 = 24;
/// Bounds of the interval, in hours: at most hourly, at least weekly.
pub const INTERVAL_HOURS_RANGE: RangeInclusive<u32> = 1..=168;
/// Days the newest copy before an update stays once the update is proven (RD-1240-34).
pub const DEFAULT_BACKUP_RETENTION_DAYS: u32 = 14;
/// Bounds of that, in days; 0 keeps the newest copy for good.
pub const BACKUP_RETENTION_DAYS_RANGE: RangeInclusive<u32> = 0..=3650;

/// The update slice of the settings document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct UpdateSettings {
    /// Whether the service checks by itself. On by default (owner, 2026-09-30): a check sends
    /// nothing but a request for a public file, and an installation that never learns about a
    /// security fix is the worse default. "Check now" works either way.
    pub update_check_enabled: bool,
    /// `stable` or `beta`; while nobody chose one, [`default_channel`] of the running build.
    pub update_channel: String,
    pub update_check_interval_hours: u32,
    /// Whether an offered update installs by itself (RD-1240-27). Off by default: replacing the
    /// program is a decision somebody takes, and it does nothing where the installation does not
    /// install itself.
    pub update_auto_install: bool,
    /// When an automatic install, and an automatic restart, may start; `None` at any time.
    pub update_auto_install_window: Option<InstallWindow>,
    /// Whether a pending restart is carried out by itself (RD-1240-32): a plugin installed or
    /// updated that runs only from the next start. Off by default, like the automatic install.
    pub restart_when_needed: bool,
    /// Days the newest database copy and archive before an update, and the newest copy before a
    /// migration, stay once that update is proven (RD-1240-34); the older ones go at once. 0
    /// keeps the newest for good.
    pub update_backup_retention_days: u32,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            update_check_enabled: true,
            update_channel: default_channel(env!("CARGO_PKG_VERSION"))
                .as_str()
                .to_owned(),
            update_check_interval_hours: DEFAULT_INTERVAL_HOURS,
            update_auto_install: false,
            update_auto_install_window: None,
            restart_when_needed: false,
            update_backup_retention_days: DEFAULT_BACKUP_RETENTION_DAYS,
        }
    }
}

/// The channel an installation reads while nobody chose one: `beta` on a build that is a
/// pre-release itself, because whoever runs 1.8.0-beta.1 is meant to be offered 1.8.0-beta.2,
/// and `stable` on every other. A chosen channel always wins, `stable` included.
#[must_use]
pub fn default_channel(running: &str) -> Channel {
    match parse_version(running) {
        Some(version) if !version.pre.is_empty() => Channel::Beta,
        _ => Channel::Stable,
    }
}

impl UpdateSettings {
    /// The chosen channel; an unknown value reads as stable, the channel that offers less.
    #[must_use]
    pub fn channel(&self) -> Channel {
        Channel::parse(&self.update_channel).unwrap_or_default()
    }

    /// The time window of the automatic install, `None` (any time) when the stored one is invalid.
    #[must_use]
    pub fn auto_install_window(&self) -> Option<InstallWindow> {
        self.update_auto_install_window
            .filter(|window| window.is_valid())
    }

    /// The days the newest backup stays after a proven update, the default when the stored
    /// value is out of range.
    #[must_use]
    pub fn backup_retention_days(&self) -> u32 {
        if BACKUP_RETENTION_DAYS_RANGE.contains(&self.update_backup_retention_days) {
            self.update_backup_retention_days
        } else {
            DEFAULT_BACKUP_RETENTION_DAYS
        }
    }

    /// The interval in hours, the default when the stored one is out of range.
    #[must_use]
    pub fn interval_hours(&self) -> u32 {
        if INTERVAL_HOURS_RANGE.contains(&self.update_check_interval_hours) {
            self.update_check_interval_hours
        } else {
            DEFAULT_INTERVAL_HOURS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_without_the_fields_reads_as_the_defaults() {
        let settings: UpdateSettings =
            serde_json::from_value(serde_json::json!({ "max_active_files": 3 })).expect("parse");
        assert_eq!(settings, UpdateSettings::default());
        assert!(settings.update_check_enabled);
        assert!(!settings.update_auto_install, "off by default (RD-1240-27)");
        assert!(!settings.restart_when_needed, "off by default (RD-1240-32)");
        assert_eq!(settings.backup_retention_days(), 14, "RD-1240-34");
        assert_eq!(
            settings.channel(),
            default_channel(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn a_pre_release_build_reads_beta_until_a_channel_is_chosen() {
        assert_eq!(default_channel("1.8.0-beta.1"), Channel::Beta);
        assert_eq!(default_channel("v1.8.0-beta.2"), Channel::Beta);
        assert_eq!(default_channel("1.8.0"), Channel::Stable);
        assert_eq!(default_channel("not a version"), Channel::Stable);
        let chosen: UpdateSettings =
            serde_json::from_value(serde_json::json!({ "update_channel": "stable" }))
                .expect("parse");
        assert_eq!(chosen.channel(), Channel::Stable);
    }

    #[test]
    fn broken_values_fall_back_to_the_safe_ones() {
        let settings = UpdateSettings {
            update_check_enabled: true,
            update_channel: "nightly".to_owned(),
            update_check_interval_hours: 0,
            update_auto_install: true,
            update_auto_install_window: Some(InstallWindow {
                start_minute: 60,
                end_minute: 60,
            }),
            restart_when_needed: true,
            update_backup_retention_days: 99_999,
        };
        assert_eq!(settings.channel(), Channel::Stable);
        assert_eq!(settings.interval_hours(), DEFAULT_INTERVAL_HOURS);
        assert_eq!(settings.auto_install_window(), None);
        assert_eq!(
            settings.backup_retention_days(),
            DEFAULT_BACKUP_RETENTION_DAYS
        );
    }
}

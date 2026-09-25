//! Quiet hours, queue completion actions and the network context (part of the
//! `service.settings` blob, so they ride the settings backup).
//!
//! These live here rather than in `rd-core` because they name `rd_limits::QuietHours`, and
//! `rd-limits` already depends on `rd-core`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What runs once the queue and post-processing have drained.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletionAction {
    #[default]
    None,
    /// Runs the configured script through the existing post-processing sandbox.
    Script,
    Standby,
    Shutdown,
}

impl CompletionAction {
    /// Whether the action powers the machine down or to sleep, and therefore needs the
    /// local approval and a countdown that can still be cancelled.
    #[must_use]
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Standby | Self::Shutdown)
    }
}

/// Shortest and longest countdown before a power action runs.
pub const MIN_COMPLETION_COUNTDOWN: u32 = 10;
pub const MAX_COMPLETION_COUNTDOWN: u32 = 3600;
pub const DEFAULT_COMPLETION_COUNTDOWN: u32 = 60;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PowerSettings {
    /// Weekly windows during which deferred work waits.
    pub quiet_hours: rd_limits::QuietHours,
    /// Hold back PAR2 repair, unpacking and uploads while quiet hours are active.
    pub quiet_hours_defer_postprocess: bool,
    /// Group notification deliveries until the quiet period ends.
    pub quiet_hours_defer_notifications: bool,
    pub completion_action: CompletionAction,
    /// Script name inside the post-processing scripts directory.
    pub completion_script: Option<String>,
    pub completion_countdown_seconds: u32,
    /// Explicit local approval for standby and shutdown. Without it a power action is
    /// configured but never runs, so a remote change alone cannot switch a machine off.
    pub power_actions_allowed: bool,
    /// Pause the queue while the machine runs on battery.
    pub pause_on_battery: bool,
    /// Pause the queue while the connection reports itself as metered.
    pub pause_on_metered: bool,
    /// Keep the machine awake while downloads or post-processing are actually running.
    pub prevent_standby: bool,
    /// Keep the display awake as well. Separate wish: a download needs no lit screen.
    pub prevent_display_standby: bool,
}

impl Default for PowerSettings {
    fn default() -> Self {
        Self {
            quiet_hours: rd_limits::QuietHours::default(),
            quiet_hours_defer_postprocess: true,
            quiet_hours_defer_notifications: true,
            completion_action: CompletionAction::None,
            completion_script: None,
            completion_countdown_seconds: DEFAULT_COMPLETION_COUNTDOWN,
            power_actions_allowed: false,
            pause_on_battery: false,
            pause_on_metered: false,
            prevent_standby: false,
            prevent_display_standby: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CompletionAction, DEFAULT_COMPLETION_COUNTDOWN, PowerSettings};

    #[test]
    fn a_blob_without_the_keys_does_nothing_on_completion() {
        let legacy: PowerSettings = serde_json::from_str("{}").expect("empty blob");
        assert_eq!(legacy.completion_action, CompletionAction::None);
        assert!(!legacy.power_actions_allowed);
        assert_eq!(
            legacy.completion_countdown_seconds,
            DEFAULT_COMPLETION_COUNTDOWN
        );
    }

    #[test]
    fn only_the_power_actions_need_an_approval() {
        assert!(CompletionAction::Standby.is_destructive());
        assert!(CompletionAction::Shutdown.is_destructive());
        assert!(!CompletionAction::Script.is_destructive());
        assert!(!CompletionAction::None.is_destructive());
    }
}

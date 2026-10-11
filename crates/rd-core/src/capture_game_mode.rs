//! The agent's game mode (RD-1240-19): while a full-screen program is in front or a named process
//! runs, the desktop agent pauses the queue or switches a bandwidth profile, and lifts it again
//! afterwards -- only what it set itself.
//!
//! Shared by both ends. The service validates the settings and decides on its side whether a
//! hold is the agent's own; the agent decides when it asks for one. Every hold is a timed one
//! ([`HOLD_MINUTES`]) the agent renews while the trigger lasts, so an agent that crashes, loses
//! its network or is put to sleep leaves nothing behind for longer than that.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::BandwidthProfileId;

/// How long one hold lasts before the agent has to renew it.
pub const HOLD_MINUTES: u32 = 15;
/// The agent renews a hold once fewer minutes than this remain: every five minutes or so, which
/// leaves room for a clock between the two machines that is a few minutes off.
pub const RENEW_BEFORE_MINUTES: i64 = 10;
/// The most process names one list holds.
pub const MAX_GAME_MODE_PROCESSES: usize = 64;
/// The longest process name, in characters (the Windows path limit, which no name reaches).
const MAX_PROCESS_NAME: usize = 260;

/// What the agent does while a trigger holds.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GameModeAction {
    /// A timed pause of the whole queue; the files it stopped start again afterwards.
    #[default]
    Pause,
    /// The bandwidth profile in `profile_id`, switched on by hand in front of the schedule.
    Profile,
}

/// When the agent steps aside, and what it does then. Off while neither trigger is set, or while
/// it is switched off.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CaptureGameMode {
    /// Switched on (RD-1240-23): the tray's "Pause while gaming" and the switch on the settings
    /// page. Off keeps the triggers for the next time. Left out, it is on, so settings stored
    /// before the switch existed mean what they meant.
    #[serde(default = "switched_on")]
    pub enabled: bool,
    /// While a program fills the screen in front (Windows: a full-screen or Direct3D program, or
    /// the presentation mode). macOS and Linux cannot tell without extra rights or a tray.
    #[serde(default)]
    pub full_screen: bool,
    /// Programs by process name, `game.exe` or `game`, compared without case and without the
    /// `.exe` or `.app` ending.
    #[serde(default)]
    pub processes: Vec<String>,
    #[serde(default)]
    pub action: GameModeAction,
    /// The profile for [`GameModeAction::Profile`].
    #[serde(default)]
    pub profile_id: Option<BandwidthProfileId>,
}

impl Default for CaptureGameMode {
    fn default() -> Self {
        Self {
            enabled: true,
            full_screen: false,
            processes: Vec::new(),
            action: GameModeAction::default(),
            profile_id: None,
        }
    }
}

fn switched_on() -> bool {
    true
}

/// Why game mode settings were not taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameModeProblem {
    /// A process name with a path separator or a control character in it, or one too long.
    ProcessInvalid,
    /// More than [`MAX_GAME_MODE_PROCESSES`] names.
    TooManyProcesses,
    /// `action: profile` without a profile.
    ProfileMissing,
}

impl GameModeProblem {
    /// The stable code the REST answer carries.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ProcessInvalid => "capture.game_mode_process_invalid",
            Self::TooManyProcesses => "capture.game_mode_too_many_processes",
            Self::ProfileMissing => "capture.game_mode_profile_missing",
        }
    }
}

impl CaptureGameMode {
    /// Whether a trigger is set at all, switched on or not: without one there is nothing for the
    /// tray's switch to switch.
    #[must_use]
    pub fn watches(&self) -> bool {
        self.full_screen || !self.processes.is_empty()
    }

    /// Whether the agent watches the desktop now: a trigger is set and game mode is switched on.
    #[must_use]
    pub fn active(&self) -> bool {
        self.enabled && self.watches()
    }

    /// The settings as stored: names trimmed, empty ones dropped, each name once.
    pub fn validated(&self) -> Result<Self, GameModeProblem> {
        let mut processes: Vec<String> = Vec::new();
        for name in &self.processes {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if name.chars().count() > MAX_PROCESS_NAME
                || name
                    .chars()
                    .any(|c| c == '/' || c == '\\' || c.is_control())
            {
                return Err(GameModeProblem::ProcessInvalid);
            }
            let key = process_key(name);
            if !processes.iter().any(|kept| process_key(kept) == key) {
                processes.push(name.to_owned());
            }
        }
        if processes.len() > MAX_GAME_MODE_PROCESSES {
            return Err(GameModeProblem::TooManyProcesses);
        }
        if self.action == GameModeAction::Profile && self.profile_id.is_none() {
            return Err(GameModeProblem::ProfileMissing);
        }
        Ok(Self {
            processes,
            ..self.clone()
        })
    }

    /// The first watched name among `running`, which are [`process_key`]s.
    #[must_use]
    pub fn running_process<'a>(
        &'a self,
        running: &std::collections::HashSet<String>,
    ) -> Option<&'a str> {
        self.processes
            .iter()
            .find(|name| running.contains(&process_key(name)))
            .map(String::as_str)
    }
}

/// A process name as both sides compare it: the file name only, lower case, without `.exe` or
/// `.app`. Windows lists `game` for `game.exe`, macOS a whole path inside `Game.app`.
#[must_use]
pub fn process_key(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let lower = file.to_lowercase();
    for ending in [".exe", ".app"] {
        if let Some(stem) = lower.strip_suffix(ending)
            && !stem.is_empty()
        {
            return stem.to_owned();
        }
    }
    lower
}

/// What holds on the service, of the kind a hold is about: a queue pause or a hand-made
/// bandwidth switch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InForce {
    Nothing,
    /// One that ends at this time; the agent's own when the time is the one it was given.
    Until(DateTime<Utc>),
    /// One without an end, which is never the agent's.
    Open,
}

impl InForce {
    /// Whether the agent may set a hold now: on nothing, or over its own one (`renews`, the end it
    /// was given last). A hold somebody ended or replaced while the agent held it is theirs from
    /// then on, so a renewal over nothing is refused too.
    #[must_use]
    pub fn may_hold(self, renews: Option<DateTime<Utc>>) -> bool {
        match (self, renews) {
            (Self::Nothing, None) => true,
            (Self::Until(until), Some(renews)) => until == renews,
            _ => false,
        }
    }

    /// Whether this is the agent's hold ending at `until`, so that it may lift it.
    #[must_use]
    pub fn is_own(self, until: DateTime<Utc>) -> bool {
        self == Self::Until(until)
    }
}

#[cfg(test)]
#[path = "capture_game_mode_tests.rs"]
mod tests;

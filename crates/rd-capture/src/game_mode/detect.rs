//! Looking at the desktop for the game mode (RD-1240-19): whether a program fills the screen in
//! front, and which processes run.
//!
//! Windows is asked in this process on every look, through `rd-win32` (RD-1400-07; before, a
//! PowerShell helper ran beside the agent for it): `SHQueryUserNotificationState`, and a ToolHelp
//! snapshot of the processes when a trigger names one. macOS lists its processes with `ps` and
//! cannot tell a full-screen program from another without the screen-recording right, which the
//! agent does not ask for. Linux has no game mode: no tray, no desktop it could judge (job
//! exclusion).
//!
//! What the answers mean is decided by the functions below, which compile and are tested
//! everywhere.

use std::collections::HashSet;

/// What one look found: process names as `rd_core::game_mode_process_key`s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    pub full_screen: bool,
    pub processes: HashSet<String>,
}

/// What one look on Windows found: `SHQueryUserNotificationState`'s answer and the executables
/// that run. `QUNS_BUSY` (2: a full-screen program, or presentation settings),
/// `QUNS_RUNNING_D3D_FULL_SCREEN` (3) and `QUNS_PRESENTATION_MODE` (4) are full screen; the rest
/// -- the desktop as usual, quiet hours, a locked screen -- are not.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn windows_seen<'a>(state: i32, executables: impl Iterator<Item = &'a str>) -> Seen {
    Seen {
        full_screen: matches!(state, 2..=4),
        processes: keys(executables),
    }
}

/// `ps -A -o comm=`: one executable per line, a whole path on macOS.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn parse_ps(output: &str) -> HashSet<String> {
    keys(output.lines())
}

fn keys<'a>(names: impl Iterator<Item = &'a str>) -> HashSet<String> {
    names
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(rd_core::game_mode_process_key)
        .collect()
}

/// Looks at the desktop on each call.
#[derive(Default)]
pub(crate) struct Detector {
    /// Logged once: Linux has no game mode, or a look failed.
    told: bool,
}

impl Detector {
    /// What the desktop shows now. A look that fails sees nothing, so nothing is held for it.
    pub(crate) async fn look(&mut self, processes_wanted: bool) -> Seen {
        match self.platform_look(processes_wanted).await {
            Ok(seen) => {
                self.told = false;
                seen
            }
            Err(error) => {
                if !self.told {
                    tracing::warn!(%error, "game mode cannot look at the desktop");
                    self.told = true;
                }
                Seen::default()
            }
        }
    }

    /// Game mode is off: nothing is watched.
    pub(crate) fn stop(&mut self) {
        self.told = false;
    }

    /// A shell that cannot answer reads as the usual desktop; a process list that cannot be read
    /// fails the look.
    #[cfg(windows)]
    async fn platform_look(&mut self, processes_wanted: bool) -> anyhow::Result<Seen> {
        tokio::task::spawn_blocking(move || {
            let state = rd_win32::user_notification_state().unwrap_or(0);
            let executables = if processes_wanted {
                rd_win32::process_names()?
            } else {
                Vec::new()
            };
            Ok::<_, anyhow::Error>(windows_seen(state, executables.iter().map(String::as_str)))
        })
        .await?
    }

    #[cfg(target_os = "macos")]
    async fn platform_look(&mut self, processes_wanted: bool) -> anyhow::Result<Seen> {
        if !processes_wanted {
            return Ok(Seen::default());
        }
        let output = tokio::process::Command::new("ps")
            .args(["-A", "-o", "comm="])
            .output()
            .await?;
        anyhow::ensure!(output.status.success(), "ps ended with {}", output.status);
        Ok(Seen {
            full_screen: false,
            processes: parse_ps(&String::from_utf8_lossy(&output.stdout)),
        })
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    async fn platform_look(&mut self, _processes_wanted: bool) -> anyhow::Result<Seen> {
        anyhow::bail!("there is no game mode on Linux")
    }
}

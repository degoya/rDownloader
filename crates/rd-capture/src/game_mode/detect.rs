//! Looking at the desktop for the game mode (RD-1240-19): whether a program fills the screen in
//! front, and which processes run.
//!
//! The workspace denies `unsafe_code`, so no system call is made from this process. Windows is
//! asked by a PowerShell helper, as `rd_power` holds its wake lock: it calls
//! `SHQueryUserNotificationState` and lists the processes every five seconds and writes one line
//! each time. It ends with the agent: `kill_on_drop`, and its next line fails on a closed pipe if
//! the agent died without killing it. macOS lists its processes with `ps` and cannot tell a
//! full-screen program from another without the screen-recording right, which the agent does not
//! ask for. Linux has no game mode: no tray, no desktop it could judge (job exclusion).
//!
//! What the lines say is read by the functions below, which compile and are tested everywhere.

use std::collections::HashSet;

/// What one look found: process names as `rd_core::game_mode_process_key`s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    pub full_screen: bool,
    pub processes: HashSet<String>,
}

/// The helper's script. One line per look: the notification state, a tab, the process names
/// joined by `|` (which no Windows file name contains).
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_HELPER: &str = r#"$ErrorActionPreference = 'Stop'
$shell = Add-Type -MemberDefinition '[DllImport("shell32.dll")] public static extern int SHQueryUserNotificationState(out int state);' -Name Shell -Namespace RDownloaderGameMode -PassThru
while ($true) {
  $state = 0
  if ($shell::SHQueryUserNotificationState([ref]$state) -ne 0) { $state = 0 }
  $names = (Get-Process | ForEach-Object { $_.ProcessName }) -join '|'
  [Console]::Out.WriteLine("$state`t$names")
  [Console]::Out.Flush()
  Start-Sleep -Seconds 5
}"#;

/// One line of the Windows helper. `QUNS_BUSY` (2: a full-screen program, or presentation
/// settings), `QUNS_RUNNING_D3D_FULL_SCREEN` (3) and `QUNS_PRESENTATION_MODE` (4) are full
/// screen; the rest -- the desktop as usual, quiet hours, a locked screen -- are not.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn parse_windows_line(line: &str) -> Option<Seen> {
    let (state, names) = line.trim_end_matches(['\r', '\n']).split_once('\t')?;
    let state: i32 = state.trim().parse().ok()?;
    Some(Seen {
        full_screen: matches!(state, 2..=4),
        processes: keys(names.split('|')),
    })
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

/// Looks at the desktop on each call; holds the Windows helper between them.
#[derive(Default)]
pub(crate) struct Detector {
    #[cfg(windows)]
    helper: Option<windows::Helper>,
    /// Not before then is a helper that ended started again: one that cannot run is not
    /// respawned on every look.
    #[cfg(windows)]
    next_start: Option<std::time::Instant>,
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

    /// Game mode is off: nothing is watched, the helper ends.
    pub(crate) fn stop(&mut self) {
        #[cfg(windows)]
        {
            self.helper = None;
            self.next_start = None;
        }
        self.told = false;
    }

    #[cfg(windows)]
    async fn platform_look(&mut self, _processes_wanted: bool) -> anyhow::Result<Seen> {
        if self.helper.as_mut().is_none_or(|helper| !helper.running()) {
            let now = std::time::Instant::now();
            if self.next_start.is_some_and(|at| now < at) {
                anyhow::bail!("the game mode helper ended; it is started again within a minute");
            }
            self.next_start = Some(now + std::time::Duration::from_secs(60));
            self.helper = Some(windows::Helper::start()?);
        }
        Ok(self
            .helper
            .as_ref()
            .and_then(windows::Helper::latest)
            .unwrap_or_default())
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

#[cfg(windows)]
mod windows {
    use rd_files::NoConsoleWindow as _;
    use tokio::io::AsyncBufReadExt as _;
    use tokio::sync::watch;

    use super::{Seen, WINDOWS_HELPER, parse_windows_line};

    /// The running helper and the last line it wrote.
    pub(super) struct Helper {
        child: tokio::process::Child,
        latest: watch::Receiver<Option<Seen>>,
    }

    impl Helper {
        pub(super) fn start() -> anyhow::Result<Self> {
            let mut child = tokio::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_HELPER])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .no_console_window()
                .spawn()?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("the game mode helper has no output"))?;
            let (sender, latest) = watch::channel(None);
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(seen) = parse_windows_line(&line) {
                        sender.send_replace(Some(seen));
                    }
                }
            });
            Ok(Self { child, latest })
        }

        /// Whether the helper still runs; one that ended is started again on the next look.
        pub(super) fn running(&mut self) -> bool {
            matches!(self.child.try_wait(), Ok(None))
        }

        pub(super) fn latest(&self) -> Option<Seen> {
            self.latest.borrow().clone()
        }
    }
}

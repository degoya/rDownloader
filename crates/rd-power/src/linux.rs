//! Linux adapter: systemd for the power actions, sysfs for the battery, NetworkManager for
//! the metered flag. Everything is optional — a headless server without systemd or
//! NetworkManager simply reports the capability as absent.

use async_trait::async_trait;

use crate::adapter::{Inhibition, PowerAdapter, PowerCapabilities, PowerState, run};

const POWER_SUPPLY: &str = "/sys/class/power_supply";

#[derive(Debug)]
pub(crate) struct LinuxAdapter {
    systemd: bool,
    battery: bool,
    /// Whether `systemd-inhibit` is on PATH; separate from `systemd`, because a container can
    /// report the one without shipping the other.
    inhibit: bool,
}

impl LinuxAdapter {
    pub(crate) fn probe() -> Self {
        Self {
            // The canonical test for "systemd is the init system running right now".
            systemd: std::path::Path::new("/run/systemd/system").is_dir(),
            battery: has_battery(),
            inhibit: which("systemd-inhibit"),
        }
    }
}

/// Whether a program is on PATH. Cheap enough to run once at probe time.
fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
    })
}

fn has_battery() -> bool {
    let Ok(entries) = std::fs::read_dir(POWER_SUPPLY) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        std::fs::read_to_string(entry.path().join("type"))
            .is_ok_and(|kind| kind.trim() == "Battery")
    })
}

/// `true` while no mains adapter is online. Read from sysfs rather than from a daemon, so
/// it works on a bare server too.
fn on_battery() -> Option<bool> {
    let entries = std::fs::read_dir(POWER_SUPPLY).ok()?;
    let mut saw_mains = false;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).ok()?;
        if kind.trim() != "Mains" {
            continue;
        }
        saw_mains = true;
        if std::fs::read_to_string(path.join("online")).ok()?.trim() == "1" {
            return Some(false);
        }
    }
    saw_mains.then_some(true)
}

/// NetworkManager's own judgement; anything but a clear "yes" counts as not metered.
async fn metered() -> Option<bool> {
    let output = run("nmcli", &["-t", "-f", "GENERAL.METERED", "device", "show"])
        .await
        .ok()?;
    let mut seen = false;
    for line in output.lines() {
        let Some((_, value)) = line.split_once(':') else {
            continue;
        };
        seen = true;
        let value = value.trim().to_ascii_lowercase();
        if value.starts_with("yes") {
            return Some(true);
        }
    }
    seen.then_some(false)
}

#[async_trait]
impl PowerAdapter for LinuxAdapter {
    fn capabilities(&self) -> PowerCapabilities {
        PowerCapabilities {
            standby: self.systemd,
            shutdown: self.systemd,
            battery: self.battery,
            // Probed on demand: nmcli may appear or disappear with the desktop session.
            metered: true,
            inhibit_standby: self.inhibit,
            // logind's idle inhibition only reaches a session that honours it; a headless
            // machine has no display to keep on. Offered on the same terms as the rest.
            inhibit_display: self.inhibit,
        }
    }

    async fn state(&self) -> PowerState {
        PowerState {
            on_battery: self.battery.then(on_battery).flatten(),
            metered: metered().await,
        }
    }

    async fn standby(&self) -> anyhow::Result<()> {
        run("systemctl", &["suspend"]).await.map(|_| ())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        run("systemctl", &["poweroff"]).await.map(|_| ())
    }

    /// Holds a logind inhibitor for as long as the helper process lives.
    ///
    /// The helper blocks on a pipe this process owns, so the inhibition ends when the child is
    /// killed *and* if the service dies without killing it: the pipe closes either way. That is
    /// the reason for the shell loop rather than `sleep infinity`.
    async fn inhibit(&self, display: bool) -> anyhow::Result<Inhibition> {
        anyhow::ensure!(self.inhibit, "systemd-inhibit is not available");
        let what = if display { "sleep:idle" } else { "sleep" };
        let child = tokio::process::Command::new("systemd-inhibit")
            .args([
                &format!("--what={what}"),
                "--who=rDownloader",
                "--why=a download is in progress",
                "--mode=block",
                "sh",
                "-c",
                "while read -r _; do :; done",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Inhibition::holding(child))
    }
}
